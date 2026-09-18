"""Async JSON-RPC client over standard I/O for the Shucked LSP server."""

import asyncio
import json
import os
from pathlib import Path
from typing import Any, Dict, List, Optional, Union


class LspClient:
    """JSON-RPC client for interacting with `shucked server` over stdio."""

    def __init__(self, binary_path: Optional[str] = None):
        if binary_path is None:
            repo_root = Path(__file__).resolve().parent.parent.parent
            binary_path = str(repo_root / "target" / "debug" / "shucked")
        self.binary_path = binary_path
        self.proc: Optional[asyncio.subprocess.Process] = None
        self._next_id = 1
        self._pending_requests: Dict[int, asyncio.Future] = {}
        self._diagnostics_queues: Dict[str, asyncio.Queue] = {}
        self._latest_diagnostics: Dict[str, List[Dict[str, Any]]] = {}
        self._all_notifications: List[Dict[str, Any]] = []
        self._read_task: Optional[asyncio.Task] = None

    async def start(self) -> None:
        """Start the shucked server process and launch reader task."""
        self.proc = await asyncio.create_subprocess_exec(
            self.binary_path,
            "server",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        self._read_task = asyncio.create_task(self._read_loop())

    async def _read_message(self) -> Optional[Dict[str, Any]]:
        """Read a single LSP framed message from stdout."""
        if not self.proc or not self.proc.stdout:
            return None
        content_length: Optional[int] = None
        while True:
            line = await self.proc.stdout.readline()
            if not line:
                return None
            line_str = line.decode("latin1").strip()
            if not line_str:
                if content_length is not None:
                    break
                continue
            if line_str.lower().startswith("content-length:"):
                content_length = int(line_str.split(":", 1)[1].strip())

        if content_length is None:
            return None

        body_bytes = await self.proc.stdout.readexactly(content_length)
        return json.loads(body_bytes.decode("utf-8"))

    async def _read_loop(self) -> None:
        """Continuously read messages and dispatch responses/notifications."""
        try:
            while True:
                msg = await self._read_message()
                if msg is None:
                    break
                self._dispatch_message(msg)
        except asyncio.CancelledError:
            pass
        except Exception:
            pass
        finally:
            for fut in self._pending_requests.values():
                if not fut.done():
                    fut.set_exception(RuntimeError("LSP server process terminated"))
            self._pending_requests.clear()


    def _dispatch_message(self, message: Dict[str, Any]) -> None:
        """Handle incoming message: route responses to futures and notifications to queues."""
        if "id" in message and "method" not in message:
            req_id = message["id"]
            if req_id in self._pending_requests:
                future = self._pending_requests.pop(req_id)
                if not future.done():
                    future.set_result(message)
        elif "method" in message:
            method = message["method"]
            self._all_notifications.append(message)
            if method == "textDocument/publishDiagnostics":
                params = message.get("params", {})
                uri = params.get("uri", "")
                diagnostics = params.get("diagnostics", [])
                self._latest_diagnostics[uri] = diagnostics
                if uri not in self._diagnostics_queues:
                    self._diagnostics_queues[uri] = asyncio.Queue()
                self._diagnostics_queues[uri].put_nowait(diagnostics)
            elif "id" in message:
                # Server-initiated request: auto-reply with empty success
                asyncio.create_task(
                    self.send_message({"jsonrpc": "2.0", "id": message["id"], "result": None})
                )

    async def send_message(self, message: Dict[str, Any]) -> None:
        """Send a JSON-RPC message with Content-Length framing."""
        if not self.proc or not self.proc.stdin:
            raise RuntimeError("LspClient process is not running")
        body = json.dumps(message).encode("utf-8")
        header = f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8")
        self.proc.stdin.write(header + body)
        await self.proc.stdin.drain()

    async def send_request(self, method: str, params: Any = None, timeout: float = 10.0) -> Any:
        """Send a request and await its response."""
        req_id = self._next_id
        self._next_id += 1
        future = asyncio.get_running_loop().create_future()
        self._pending_requests[req_id] = future

        msg: Dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
        }
        if params is not None:
            msg["params"] = params

        await self.send_message(msg)
        response = await asyncio.wait_for(future, timeout=timeout)
        if "error" in response:
            raise RuntimeError(f"LSP error in {method}: {response['error']}")
        return response.get("result")


    async def send_notification(self, method: str, params: Any = None) -> None:
        """Send a notification."""
        msg: Dict[str, Any] = {
            "jsonrpc": "2.0",
            "method": method,
        }
        if params is not None:
            msg["params"] = params
        await self.send_message(msg)

    async def initialize(
        self,
        capabilities: Optional[Dict[str, Any]] = None,
        initialization_options: Optional[Dict[str, Any]] = None,
        root_uri: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Perform LSP initialize handshake."""
        params: Dict[str, Any] = {
            "capabilities": capabilities if capabilities is not None else {},
        }
        if root_uri is not None:
            params["rootUri"] = root_uri
        if initialization_options is not None:
            params["initializationOptions"] = initialization_options
        result = await self.send_request("initialize", params)
        return result


    async def initialized(self) -> None:
        """Send LSP initialized notification."""
        await self.send_notification("initialized", {})

    async def open_document(
        self,
        uri: str,
        language_id: str = "shellscript",
        text: str = "",
        version: int = 1,
    ) -> None:
        """Send textDocument/didOpen notification."""
        if uri not in self._diagnostics_queues:
            self._diagnostics_queues[uri] = asyncio.Queue()
        await self.send_notification(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                }
            },
        )

    async def change_document(
        self,
        uri: str,
        text: str,
        version: int,
    ) -> None:
        """Send textDocument/didChange notification with full document replacement."""
        await self.send_notification(
            "textDocument/didChange",
            {
                "textDocument": {
                    "uri": uri,
                    "version": version,
                },
                "contentChanges": [{"text": text}],
            },
        )

    async def hover(self, uri: str, line: int, character: int) -> Optional[Dict[str, Any]]:
        """Send textDocument/hover request."""
        return await self.send_request(
            "textDocument/hover",
            {
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            },
        )

    async def completion(self, uri: str, line: int, character: int) -> Any:
        """Send textDocument/completion request."""
        return await self.send_request(
            "textDocument/completion",
            {
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            },
        )

    async def formatting(
        self,
        uri: str,
        tab_size: int = 2,
        insert_spaces: bool = True,
    ) -> Optional[List[Dict[str, Any]]]:
        """Send textDocument/formatting request."""
        return await self.send_request(
            "textDocument/formatting",
            {
                "textDocument": {"uri": uri},
                "options": {
                    "tabSize": tab_size,
                    "insertSpaces": insert_spaces,
                },
            },
        )

    async def range_formatting(
        self,
        uri: str,
        start_line: int,
        start_char: int,
        end_line: int,
        end_char: int,
        tab_size: int = 2,
        insert_spaces: bool = True,
    ) -> Optional[List[Dict[str, Any]]]:
        """Send textDocument/rangeFormatting request."""
        return await self.send_request(
            "textDocument/rangeFormatting",
            {
                "textDocument": {"uri": uri},
                "range": {
                    "start": {"line": start_line, "character": start_char},
                    "end": {"line": end_line, "character": end_char},
                },
                "options": {
                    "tabSize": tab_size,
                    "insertSpaces": insert_spaces,
                },
            },
        )

    async def code_action(
        self,
        uri: str,
        range: Union[Dict[str, Any], tuple, list],
        diagnostics: Optional[List[Dict[str, Any]]] = None,
        only: Optional[List[str]] = None,
    ) -> Optional[List[Dict[str, Any]]]:
        """Send textDocument/codeAction request."""
        if isinstance(range, (tuple, list)):
            start_l, start_c, end_l, end_c = range
            range_dict = {
                "start": {"line": start_l, "character": start_c},
                "end": {"line": end_l, "character": end_c},
            }
        else:
            range_dict = range

        context: Dict[str, Any] = {
            "diagnostics": diagnostics or [],
        }
        if only is not None:
            context["only"] = only

        return await self.send_request(
            "textDocument/codeAction",
            {
                "textDocument": {"uri": uri},
                "range": range_dict,
                "context": context,
            },
        )

    async def definition(self, uri: str, line: int, character: int) -> Any:
        """Send textDocument/definition request."""
        return await self.send_request(
            "textDocument/definition",
            {
                "textDocument": {"uri": uri},
                "position": {"line": line, "character": character},
            },
        )

    async def wait_for_diagnostics(self, uri: str, timeout: float = 5.0) -> List[Dict[str, Any]]:
        """Wait for diagnostics to be published for uri and return them."""
        if uri not in self._diagnostics_queues:
            self._diagnostics_queues[uri] = asyncio.Queue()
        try:
            return await asyncio.wait_for(self._diagnostics_queues[uri].get(), timeout=timeout)
        except asyncio.TimeoutError:
            # Fall back to latest known diagnostics if already received
            if uri in self._latest_diagnostics:
                return self._latest_diagnostics[uri]
            raise TimeoutError(f"Timed out waiting for diagnostics on {uri}")

    async def shutdown_and_exit(self) -> None:
        """Send shutdown request and exit notification, then cleanly terminate process."""
        if self.proc is None or self.proc.returncode is not None:
            return
        try:
            await asyncio.wait_for(self.send_request("shutdown", None), timeout=2.0)
        except Exception:
            pass
        try:
            await self.send_notification("exit", None)
        except Exception:
            pass


        if self._read_task:
            self._read_task.cancel()
            try:
                await self._read_task
            except asyncio.CancelledError:
                pass

        try:
            await asyncio.wait_for(self.proc.wait(), timeout=3.0)
        except (asyncio.TimeoutError, Exception):
            if self.proc.returncode is None:
                self.proc.kill()
                await self.proc.wait()

"""Python client for the in-editor test bridge (``bridge/runner.cjs``).

The bridge runs inside the VS Code extension host and exposes the extension API
over newline-delimited JSON on an authenticated loopback socket. Arguments that
must become API objects are tagged: see :func:`uri`, :func:`position`,
:func:`range_`, and :func:`selection`. Results come back as plain JSON: URIs are
strings, positions are ``{"line", "character"}``, Markdown is its text, and
snippets are ``{"snippet": value}``.
"""

from __future__ import annotations

import contextlib
import itertools
import json
import re
import socket
import time
from pathlib import Path
from typing import Any

JSON = Any
_MARKDOWN_ESCAPE = re.compile(r"\\([\\`*_{}\[\]()#+\-.!|>~])")


class BridgeError(RuntimeError):
    """An API call failed inside the extension host."""

    def __init__(self, method: str, message: str, stack: str | None) -> None:
        super().__init__(f"{method}: {message}")
        self.stack = stack


def uri(value: str | Path) -> dict[str, str]:
    """Tag a file path or URI string for conversion to ``vscode.Uri``."""
    if isinstance(value, Path) or not ("://" in value or value.startswith("untitled:")):
        return {"$uri": Path(value).resolve().as_uri()}
    return {"$uri": value}


def position(line: int, character: int) -> dict[str, list[int]]:
    return {"$position": [line, character]}


def range_(start_line: int, start_character: int, end_line: int, end_character: int) -> dict[str, list[int]]:
    return {"$range": [start_line, start_character, end_line, end_character]}


def file_uri(path: str | Path) -> str:
    return Path(path).resolve().as_uri()


def label(item: dict[str, JSON]) -> str:
    """Return the display label of an encoded completion item."""
    value = item.get("label")
    return value["label"] if isinstance(value, dict) else str(value)


def inserted_text(item: dict[str, JSON]) -> str:
    """Return the text a completion item inserts, whatever form it uses."""
    insert = item.get("insertText")
    if isinstance(insert, dict):
        return str(insert.get("snippet", ""))
    if isinstance(insert, str):
        return insert
    edit = item.get("textEdit")
    if isinstance(edit, dict) and "newText" in edit:
        return str(edit["newText"])
    return label(item)


def plain_markdown(text: str) -> str:
    """Drop Markdown backslash escapes and non-breaking space entities from hover text."""
    return _MARKDOWN_ESCAPE.sub(r"\1", text.replace("&nbsp;", " "))


def diagnostic_code(diagnostic: dict[str, JSON]) -> str | None:
    code = diagnostic.get("code")
    if isinstance(code, dict):
        code = code.get("value")
    return None if code is None else str(code)


class Bridge:
    """Synchronous client; one request is in flight at a time.

    A call that times out leaves the connection usable: its late response is
    recognised by id and discarded when the next call reads the stream.
    """

    def __init__(self, port: int, token: str, timeout: float = 30.0) -> None:
        self.timeout = timeout
        self._socket = socket.create_connection(("127.0.0.1", port), timeout=timeout)
        self._buffer = b""
        self._ids = itertools.count(1)
        self._configuration_changes: list[tuple[str, str, str, str | None, JSON]] = []
        self._send({"token": token})
        if not json.loads(self._read_line(time.monotonic() + timeout) or b"{}").get("ready"):
            raise ConnectionError("The editor bridge rejected the connection")

    def _send(self, message: dict[str, JSON]) -> None:
        self._socket.sendall((json.dumps(message) + "\n").encode("utf-8"))

    def _read_line(self, deadline: float) -> bytes:
        while b"\n" not in self._buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("timed out waiting for the editor bridge")
            self._socket.settimeout(remaining)
            try:
                chunk = self._socket.recv(1 << 16)
            except TimeoutError as error:
                raise TimeoutError("timed out waiting for the editor bridge") from error
            if not chunk:
                raise ConnectionError("The editor bridge closed the connection")
            self._buffer += chunk
        line, _, self._buffer = self._buffer.partition(b"\n")
        return line

    def call(self, method: str, timeout: float | None = None, **params: JSON) -> JSON:
        request_id = next(self._ids)
        deadline = time.monotonic() + (self.timeout if timeout is None else timeout)
        self._send({"id": request_id, "method": method, "params": params})
        while True:
            response = json.loads(self._read_line(deadline))
            if response.get("id") == request_id:
                break
        if "error" in response:
            error = response["error"]
            raise BridgeError(method, error.get("message", "unknown error"), error.get("stack"))
        return response.get("result")

    def close(self) -> None:
        with contextlib.suppress(OSError):
            self._socket.close()

    def shutdown(self, code: int = 0, message: str | None = None) -> None:
        try:
            self.call("shutdown", timeout=10, code=code, message=message)
        except (OSError, ConnectionError, BridgeError):
            pass
        finally:
            self.close()

    # -- Workbench state -------------------------------------------------

    def ping(self) -> dict[str, JSON]:
        return self.call("ping")

    def execute(self, command: str, *args: JSON, timeout: float | None = None) -> JSON:
        return self.call("executeCommand", timeout=timeout, command=command, args=list(args))

    def start(self, command: str, *args: JSON) -> None:
        """Run a command without waiting for it, e.g. when it awaits a notification."""
        self.call("startCommand", command=command, args=list(args))

    def commands(self, filter_internal: bool = False) -> list[str]:
        return self.call("commands", filterInternal=filter_internal)

    def extension(self, extension_id: str) -> dict[str, JSON] | None:
        return self.call("extension", id=extension_id)

    def activate_extension(self, extension_id: str) -> dict[str, JSON]:
        return self.call("activateExtension", id=extension_id)

    def evaluate(self, code: str, **args: JSON) -> JSON:
        """Run JavaScript in the extension host; use only when no method fits."""
        return self.call("evaluate", code=code, args=args)

    # -- Documents and editors -------------------------------------------

    def open(self, path: str | Path, show: bool = True) -> dict[str, JSON]:
        document = self.call("openDocument", uri=file_uri(path))
        if show:
            self.show(document["uri"])
        return document

    def open_untitled(self, language: str, content: str, show: bool = True) -> dict[str, JSON]:
        document = self.call("openDocument", language=language, content=content)
        if show:
            self.show(document["uri"])
        return document

    def show(self, document_uri: str, preview: bool = False) -> dict[str, JSON]:
        return self.call("showDocument", uri=document_uri, preview=preview)

    def document(self, document_uri: str) -> dict[str, JSON]:
        return self.call("document", uri=document_uri)

    def text(self, document_uri: str) -> str:
        return self.document(document_uri)["text"]

    def replace_text(self, document_uri: str, text: str) -> bool:
        return self.call("replaceText", uri=document_uri, text=text)

    def apply_edit(self, document_uri: str, start: dict[str, int], end: dict[str, int], text: str) -> bool:
        """Replace one range, given as encoded positions (``{"line", "character"}``)."""
        return self.call("applyEdit", uri=document_uri, range=[start["line"], start["character"], end["line"], end["character"]], text=text)

    def set_selections(self, document_uri: str | None, *selections: tuple[int, int, int, int]) -> JSON:
        """Place one or more cursors or selections as (anchor line, anchor char, active line, active char)."""
        return self.call("setSelections", uri=document_uri, selections=[list(item) for item in selections])

    def set_cursor(self, document_uri: str | None, line: int, character: int) -> JSON:
        return self.call("setSelection", uri=document_uri, anchor=[line, character])

    def active_editor(self) -> dict[str, JSON]:
        """URI, selections, and selected text of the active editor; raises when there is none."""
        editor = self.call("activeEditor")
        if editor is None:
            raise BridgeError("activeEditor", "no active text editor", None)
        return editor

    def close_all_editors(self) -> None:
        """Revert unsaved changes and close every editor without prompting."""
        self.call("resetEditors")

    # -- Language features -----------------------------------------------

    def diagnostics(self, document_uri: str) -> list[dict[str, JSON]]:
        return self.call("diagnostics", uri=document_uri)

    def diagnostic_codes(self, document_uri: str) -> list[str | None]:
        return [diagnostic_code(item) for item in self.diagnostics(document_uri)]

    def completions(self, document_uri: str, line: int, character: int, trigger: str | None = None) -> dict[str, JSON]:
        args: list[JSON] = [{"$uri": document_uri}, position(line, character)]
        if trigger is not None:
            args.append(trigger)
        return self.execute("vscode.executeCompletionItemProvider", *args) or {"items": [], "isIncomplete": False}

    def completion_labels(self, document_uri: str, line: int, character: int) -> list[str]:
        return [label(item) for item in self.completions(document_uri, line, character)["items"]]

    def hover_text(self, document_uri: str, line: int, character: int) -> list[str]:
        hovers = self.execute("vscode.executeHoverProvider", {"$uri": document_uri}, position(line, character)) or []
        return [plain_markdown(str(content)) for hover in hovers for content in hover.get("contents", [])]

    def semantic_tokens(self, document_uri: str) -> list[int]:
        tokens = self.execute("vscode.provideDocumentSemanticTokens", {"$uri": document_uri})
        return list(tokens.get("data", [])) if tokens else []

    def semantic_legend(self, document_uri: str) -> dict[str, list[str]]:
        return self.execute("vscode.provideDocumentSemanticTokensLegend", {"$uri": document_uri}) or {}

    def format_edits(self, document_uri: str, tab_size: int = 2, insert_spaces: bool = True) -> list[dict[str, JSON]]:
        options = {"tabSize": tab_size, "insertSpaces": insert_spaces}
        return self.execute("vscode.executeFormatDocumentProvider", {"$uri": document_uri}, options) or []

    def code_actions(
        self, document_uri: str, line: int, character: int, end_line: int | None = None, end_character: int | None = None
    ) -> list[dict[str, JSON]]:
        target = range_(line, character, line if end_line is None else end_line, character if end_character is None else end_character)
        return self.execute("vscode.executeCodeActionProvider", {"$uri": document_uri}, target) or []

    def document_symbols(self, document_uri: str) -> list[dict[str, JSON]]:
        return self.execute("vscode.executeDocumentSymbolProvider", {"$uri": document_uri}) or []

    def definitions(self, document_uri: str, line: int, character: int) -> list[dict[str, JSON]]:
        return self.execute("vscode.executeDefinitionProvider", {"$uri": document_uri}, position(line, character)) or []

    # -- Configuration ---------------------------------------------------

    def setting(self, section: str, key: str, scope: str | None = None) -> JSON:
        return self.call("getConfiguration", section=section, key=key, scope=scope)

    def inspect_setting(self, section: str, key: str, scope: str | None = None) -> dict[str, JSON]:
        return self.call("inspectConfiguration", section=section, key=key, scope=scope)

    def update_setting(self, section: str, key: str, value: JSON, target: str = "global", scope: str | None = None) -> None:
        """Change a setting and remember its previous value for :meth:`restore_settings`."""
        inspected = self.inspect_setting(section, key, scope)
        field = {"global": "globalValue", "workspace": "workspaceValue", "workspaceFolder": "workspaceFolderValue"}[target]
        self._configuration_changes.append((section, key, target, scope, (inspected or {}).get(field)))
        self.call("updateConfiguration", section=section, key=key, value=value, target=target, scope=scope)

    def restore_settings(self) -> None:
        while self._configuration_changes:
            section, key, target, scope, value = self._configuration_changes.pop()
            self.call("updateConfiguration", section=section, key=key, value=value, target=target, scope=scope)

    # -- Terminals ----------------------------------------------------------

    def terminals(self) -> list[dict[str, JSON]]:
        return self.call("terminals")

    def terminal_send_text(self, name: str, text: str, add_new_line: bool = True) -> None:
        self.call("terminalSendText", name=name, text=text, addNewLine=add_new_line)

    def dispose_terminals(self, names: list[str] | None = None) -> None:
        self.call("disposeTerminals", names=names)

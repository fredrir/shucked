"""A private virtual X display for editor tests on Linux hosts without a screen."""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from dataclasses import dataclass


@dataclass
class Display:
    name: str | None
    process: subprocess.Popen[bytes] | None = None

    def environment(self) -> dict[str, str]:
        return {"DISPLAY": self.name} if self.name else {}

    def stop(self) -> None:
        if self.process and self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)


def start(headed: bool) -> Display:
    """Start Xvfb unless the run is headed or the platform has a native window server."""
    if not sys.platform.startswith("linux") or headed:
        return Display(os.environ.get("DISPLAY"))
    xvfb = shutil.which("Xvfb")
    if not xvfb:
        raise RuntimeError("Xvfb is required for editor tests on Linux (install the xvfb package, or pass --headed with a DISPLAY)")
    read, write = os.pipe()
    process = subprocess.Popen(
        [xvfb, "-displayfd", str(write), "-screen", "0", "1280x900x24", "-nolisten", "tcp"],
        pass_fds=(write,),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    os.close(write)
    with os.fdopen(read) as stream:
        number = stream.readline().strip()
    if not number:
        process.kill()
        raise RuntimeError("Xvfb did not report a display number")
    return Display(f":{number}", process)

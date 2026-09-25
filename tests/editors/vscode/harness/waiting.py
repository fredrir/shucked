"""Polling helpers for asynchronous editor and shell state."""

from __future__ import annotations

import time
from collections.abc import Callable
from typing import TypeVar

T = TypeVar("T")


class WaitTimeout(AssertionError):
    """Raised when a condition never became true before its deadline."""


def wait_until(
    description: str,
    probe: Callable[[], T],
    timeout: float = 20.0,
    interval: float = 0.15,
) -> T:
    """Poll ``probe`` until it returns a truthy value and return that value.

    Exceptions raised by the probe are treated as "not yet" and the last one is
    reported if the deadline passes, so transient editor states do not hide the
    real reason a condition was never met.
    """
    deadline = time.monotonic() + timeout
    last_error: BaseException | None = None
    last_value: object = None
    while True:
        try:
            value = probe()
            if value:
                return value
            last_value = value
        except Exception as error:  # noqa: BLE001 - reported below when the wait fails
            last_error = error
        if time.monotonic() >= deadline:
            detail = f": {last_error!r}" if last_error else f" (last value: {last_value!r})"
            raise WaitTimeout(f"{description} timed out after {timeout:.1f}s{detail}")
        time.sleep(interval)


def stays_false(probe: Callable[[], object], duration: float, interval: float = 0.1) -> bool:
    """Return True when ``probe`` stays falsy for the whole ``duration``."""
    deadline = time.monotonic() + duration
    while time.monotonic() < deadline:
        if probe():
            return False
        time.sleep(interval)
    return True

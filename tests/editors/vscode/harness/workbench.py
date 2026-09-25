"""Page helpers for driving the VS Code workbench with real input events.

Use these where the behaviour under test lives in the workbench itself: key
bindings and their ``when`` clauses, the suggestion popup, quick picks, the
status bar, notifications, and inline (ghost text) suggestions. Everything the
extension API can observe directly belongs in bridge-level tests instead.
"""

from __future__ import annotations

import re
from typing import TYPE_CHECKING

from .waiting import WaitTimeout, wait_until

if TYPE_CHECKING:
    from playwright.sync_api import Locator, Page

    from .bridge import Bridge

SUGGEST = ".editor-widget.suggest-widget.visible"
QUICK_INPUT = ".quick-input-widget"


class Workbench:
    def __init__(self, page: Page, bridge: Bridge) -> None:
        self.page = page
        self.bridge = bridge

    # -- Editor input ------------------------------------------------------

    def focus_editor(self) -> None:
        self.bridge.execute("workbench.action.focusActiveEditorGroup")
        wait_until("editor focus", lambda: self.page.evaluate("() => !!document.activeElement?.closest('.monaco-editor')"), timeout=10)

    def type(self, text: str, delay: float = 30) -> None:
        self.page.keyboard.type(text, delay=delay)

    def press(self, key: str) -> None:
        self.page.keyboard.press(key)

    # -- Suggestion popup --------------------------------------------------

    def suggest_widget(self) -> Locator:
        return self.page.locator(SUGGEST)

    def suggest_visible(self) -> bool:
        return self.suggest_widget().count() > 0 and self.suggest_widget().first.is_visible()

    def suggestions(self) -> list[str]:
        rows = self.page.locator(f"{SUGGEST} .monaco-list-row .label-name")
        return [text.strip() for text in rows.all_inner_texts()]

    def focused_suggestion(self) -> str | None:
        row = self.page.locator(f"{SUGGEST} .monaco-list-row.focused .label-name")
        return row.first.inner_text().strip() if row.count() else None

    def wait_for_suggestions(self, description: str, predicate=lambda labels: bool(labels), timeout: float = 15.0) -> list[str]:
        def matching() -> list[str] | None:
            if not self.suggest_visible():
                return None
            labels = self.suggestions()
            return labels if predicate(labels) else None

        return wait_until(description, matching, timeout=timeout)

    # -- Inline suggestions -------------------------------------------------

    def ghost_text(self) -> str:
        parts = self.page.locator(".monaco-editor .ghost-text-decoration, .monaco-editor .ghost-text-decoration-preview")
        return "".join(parts.all_text_contents())

    # -- Quick input and commands -------------------------------------------

    def quick_input(self) -> Locator:
        return self.page.locator(QUICK_INPUT)

    def quick_input_visible(self) -> bool:
        return self.quick_input().is_visible()

    def quick_input_labels(self) -> list[str]:
        rows = self.page.locator(f"{QUICK_INPUT} .monaco-list-row .label-name")
        return [text.strip() for text in rows.all_inner_texts()]

    def pick(self, label: str, timeout: float = 15.0) -> None:
        """Choose the quick pick row whose label is exactly ``label``."""
        wait_until("quick input", self.quick_input_visible, timeout=timeout)
        row = self.page.locator(
            f"{QUICK_INPUT} .monaco-list-row", has=self.page.locator(".label-name", has_text=re.compile(rf"^\s*{re.escape(label)}\s*$"))
        )
        wait_until(f"quick pick row {label!r}", lambda: row.count() > 0, timeout=timeout)
        row.first.click()

    def enter_text(self, text: str, timeout: float = 15.0) -> None:
        """Type into the open quick input box and accept it."""
        wait_until("quick input", self.quick_input_visible, timeout=timeout)
        box = self.page.locator(f"{QUICK_INPUT} input")
        box.fill(text)
        box.press("Enter")

    def _palette_row(self, title: str, timeout: float) -> Locator:
        """Open the command palette filtered to ``title`` and return that command's row."""
        self.press("F1")
        wait_until("command palette", self.quick_input_visible, timeout=timeout)
        self.page.locator(f"{QUICK_INPUT} input").fill(f">{title}")
        exact = re.compile(rf"^\s*{re.escape(title)}\s*$")
        row = self.page.locator(f"{QUICK_INPUT} .monaco-list-row", has=self.page.locator(".label-name", has_text=exact))
        wait_until(f"command {title!r} in the palette", lambda: row.count() > 0, timeout=timeout)
        return row.first

    def run_command(self, title: str, timeout: float = 15.0) -> None:
        """Run a command through the command palette by its exact visible title."""
        self._palette_row(title, timeout).click()

    def palette_has(self, title: str, timeout: float = 15.0) -> bool:
        """True when the command palette offers a command with exactly this title."""
        try:
            self._palette_row(title, timeout)
            return True
        except WaitTimeout:
            return False
        finally:
            self.press("Escape")

    # -- Status bar and notifications ---------------------------------------

    def status_items(self) -> list[str]:
        items = self.page.locator("#workbench\\.parts\\.statusbar .statusbar-item")
        return [text.strip() for text in items.all_inner_texts() if text.strip()]

    def status_item(self, pattern: str) -> Locator:
        return self.page.locator("#workbench\\.parts\\.statusbar .statusbar-item", has_text=re.compile(pattern)).first

    def notifications(self) -> list[str]:
        messages = self.page.locator(".notifications-toasts .notification-list-item-message")
        return [text.strip() for text in messages.all_inner_texts()]

    def clear_notifications(self) -> None:
        self.bridge.execute("notifications.clearAll")

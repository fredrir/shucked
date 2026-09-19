#!/usr/bin/env python3
"""Compatibility entry point for the target-host runtime verifier."""
import runpy
from pathlib import Path
runpy.run_path(str(Path(__file__).with_name('verify-unix.py')), run_name='__main__')

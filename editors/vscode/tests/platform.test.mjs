import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { binaryPlatform, hostTarget } from "../platform.mjs";

test("targets the workspace host including ARM and Alpine", () => {
  assert.equal(hostTarget("linux", "arm64", false), "linux-arm64");
  assert.equal(hostTarget("linux", "x64", true), "alpine-x64");
  assert.equal(hostTarget("darwin", "arm64", false), "darwin-arm64");
  assert.equal(hostTarget("win32", "arm64", false), "win32-arm64");
  assert.equal(hostTarget("linux", "arm", false), "linux-armhf");
  assert.throws(() => hostTarget("freebsd", "x64", false));
});

test("reads executable architecture without launching it", () => {
  const root = mkdtempSync(join(tmpdir(), "shucked-platform-"));
  const file = join(root, "binary");
  try {
    const elf = Buffer.alloc(64);
    elf.set([0x7f, 0x45, 0x4c, 0x46, 2, 1]);
    elf.writeUInt16LE(183, 18);
    writeFileSync(file, elf);
    assert.deepEqual(binaryPlatform(file), { platform: "linux", arch: "arm64" });
    const macho = Buffer.alloc(32);
    macho.writeUInt32LE(0xfeedfacf, 0);
    macho.writeUInt32LE(0x01000007, 4);
    writeFileSync(file, macho);
    assert.deepEqual(binaryPlatform(file), { platform: "darwin", arch: "x64" });
    const pe = Buffer.alloc(256);
    pe.writeUInt16LE(0x5a4d, 0);
    pe.writeUInt32LE(128, 60);
    pe.writeUInt32LE(0x00004550, 128);
    pe.writeUInt16LE(0xaa64, 132);
    writeFileSync(file, pe);
    assert.deepEqual(binaryPlatform(file), { platform: "win32", arch: "arm64" });
    writeFileSync(file, "#!/bin/sh\necho never run");
    assert.equal(binaryPlatform(file), undefined);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

import * as fs from "node:fs";

export function hostTarget(platform = process.platform, arch = process.arch, alpine = fs.existsSync("/etc/alpine-release")) {
  const family = platform === "linux" && alpine ? "alpine" : platform;
  const cpu = arch === "arm" ? "armhf" : arch;
  const target = `${family}-${cpu}`;
  if (!["darwin-x64", "darwin-arm64", "linux-x64", "linux-arm64", "linux-armhf", "alpine-x64", "alpine-arm64", "win32-x64", "win32-arm64"].includes(target)) {
    throw new Error(`Unsupported Shucked platform: ${target}`);
  }
  return target;
}

export function binaryPlatform(filePath) {
  const fd = fs.openSync(filePath, "r");
  try {
    const header = Buffer.alloc(4096);
    const size = fs.readSync(fd, header, 0, header.length, 0);
    if (size < 20) {
      return undefined;
    }
    if (header.subarray(0, 4).equals(Buffer.from([0x7f, 0x45, 0x4c, 0x46]))) {
      const machine = header[5] === 2 ? header.readUInt16BE(18) : header.readUInt16LE(18);
      return { platform: "linux", arch: { 62: "x64", 183: "arm64", 40: "arm" }[machine] };
    }
    if (header.readUInt32LE(0) === 0xfeedfacf) {
      return { platform: "darwin", arch: { 0x01000007: "x64", 0x0100000c: "arm64" }[header.readUInt32LE(4)] };
    }
    if (header.readUInt16LE(0) === 0x5a4d && size >= 64) {
      const offset = header.readUInt32LE(60);
      if (offset + 6 <= size && header.readUInt32LE(offset) === 0x00004550) {
        return { platform: "win32", arch: { 0x8664: "x64", 0xaa64: "arm64" }[header.readUInt16LE(offset + 4)] };
      }
    }
    return undefined;
  } finally {
    fs.closeSync(fd);
  }
}

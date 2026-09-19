export function hostTarget(platform?: string, arch?: string, alpine?: boolean): string;
export function binaryPlatform(filePath: string): { platform: string; arch: string | undefined } | undefined;

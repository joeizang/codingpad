// Fallback Bun and Node globals when @types/bun is unavailable, for example
// while offline or before the first package install completes. The shared
// shadow project prefers @types/bun when it can install it.
declare const Bun: {
  version: string;
  file(path: string | URL): any;
  write(
    path: string | URL,
    data: string | Uint8Array | ArrayBuffer | Blob,
  ): Promise<number>;
  [key: string]: any;
};

type Buffer = Uint8Array;
declare const Buffer: {
  from(data: string | ArrayBuffer | Uint8Array, encoding?: string): Buffer;
  alloc(size: number): Buffer;
  [key: string]: any;
};

declare const process: {
  argv: string[];
  env: Record<string, string | undefined>;
  platform: string;
  stdout: { write(data: string | Uint8Array): void };
  stderr: { write(data: string | Uint8Array): void };
  [key: string]: any;
};

declare const __dirname: string;
declare const __filename: string;

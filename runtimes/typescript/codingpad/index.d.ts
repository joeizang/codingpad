/** Rich HTML markup rendered in the Results panel. */
export class Html {
  constructor(markup: string);
  readonly markup: string;
}

/** An image rendered in the Results panel. */
export class Image {
  constructor(src: string, mime: string, name?: string);
  readonly src: string;
  readonly mime: string;
  readonly name?: string;
}

/** A value whose dump updates whenever `.value` is assigned. */
export class Live<T> {
  constructor(value: T);
  get value(): T;
  set value(value: T);
}

/** Show a rich value tree in the Results panel. */
export function dump(value: unknown): void;

/** Render trusted markup in the Results panel. */
export function html(markup: string): Html;

/** Render a path or byte string as an image. */
export function image(
  source: string | Uint8Array | ArrayBuffer,
  mime?: string,
): Image;

/** Return a value whose dump updates when `.value` is assigned. */
export function live<T>(value: T): Live<T>;

/** Compute a value once per key in this script's warm kernel. */
export function cache<T>(key: unknown, factory: () => T): T;

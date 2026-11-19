let sink: ((value: unknown) => void) | undefined;

export function dump(value: unknown): void {
  if (sink) sink(value);
  else console.log(value);
}

export function setDumpSink(send: (value: unknown) => void): void {
  sink = send;
}

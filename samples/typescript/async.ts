// Top-level await works in CodingPad scripts.
async function square(value: number): Promise<number> {
  await Bun.sleep(50);
  return value * value;
}

await Promise.all([2, 3, 4].map(square));

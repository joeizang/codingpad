import { dump } from "codingpad";

const words = ["bun", "script", "dump", "script"];
const counts = new Map<string, number>();
for (const word of words) {
  counts.set(word, (counts.get(word) ?? 0) + 1);
}

dump([...new Set(words)]);
dump(Array.from(counts, ([word, count]) => ({ word, count })));

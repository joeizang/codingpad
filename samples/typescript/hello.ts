// Use console.log for text and dump() to inspect a typed value.
import { dump } from "codingpad";

const learner: string = "TypeScript";
console.log(`Hello, ${learner}!`);
dump({ language: learner, runtime: "Bun" });

// CodingPad also dumps the final expression in a script.
6 * 7;

import { cache, dump } from "codingpad";

function buildSquares() {
  console.log("Computing squares");
  return [1, 2, 3, 4, 5].map((value) => value ** 2);
}

const squares = cache("squares", buildSquares);
// The same key reuses the value. Rerun the tab to see it stay cached.
const reused = squares === cache("squares", buildSquares);
dump({ squares, reused });

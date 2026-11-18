# Collections and iteration

from collections import Counter

words = "the quick brown fox jumps over the lazy dog the end".split()
counts = Counter(words)

dump(dict(sorted(counts.items(), key=lambda item: (-item[1], item[0]))))

# Python syntax stays ordinary Python; use it exactly as in a normal script.
uppercase = [word.upper() for word in words if len(word) > 3]
dump(uppercase)
dump({"distinct words": len(counts), "total words": len(words)})

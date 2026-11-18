# Functions and cache()

def average(values):
    """Return the mean of a non-empty sequence."""
    return sum(values) / len(values)

# cache() keeps a slow result between runs of this tab. Change the key or press
# Reset to recompute it.
squares = cache("squares", lambda: [value * value for value in range(10)])

dump({"squares": squares, "average": average(squares)})

# A failed factory is not cached, so the next run can retry it.
dump({"answer": cache("answer", lambda: 6 * 7)})

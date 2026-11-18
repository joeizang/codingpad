# Live values

from codingpad import live

status = live("starting")
dump(status)

# Changing .value updates the dump in place while the script is still running.
status.value = "loading rows"
rows = [{"step": step, "value": step * step} for step in range(1, 6)]
status.value = f"loaded {len(rows)} rows"

dump(rows)

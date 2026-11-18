# Text, dates and JSON

import json
from datetime import date, timedelta

today = date(2026, 11, 18)
dump(
    {
        "today": today.isoformat(),
        "next week": (today + timedelta(days=7)).isoformat(),
        "weekday": today.strftime("%A"),
    }
)

payload = {"user": "ada", "roles": ["admin", "reader"]}
encoded = json.dumps(payload, indent=2)
print(encoded)

dump(json.loads(encoded))

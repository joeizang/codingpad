# Values and dumps

# dump() accepts any Python value, including nested containers.
profile = {
    "name": "Ada",
    "languages": ["Python", "C#", "SQL"],
    "active": True,
    "scores": {"first": 10, "second": 20},
}
dump(profile)

# A list of records becomes a sortable table.
dump(
    [
        {"name": "Ada", "score": 10},
        {"name": "Grace", "score": 20},
    ]
)

# Strings, numbers and None keep their type.
dump(["text", 3.14, None, False])

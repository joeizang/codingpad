# Tables from ordinary Python data

rows = [
    {"name": "Espresso", "price": 2.50, "caffeine": 63},
    {"name": "Filter coffee", "price": 3.00, "caffeine": 95},
    {"name": "Green tea", "price": 2.80, "caffeine": 28},
]

dump(rows)

# Tables virtualise their rows; scroll Results to read every record.
dump(
    [
        {"index": index, "square": index * index, "root": index**0.5}
        for index in range(1, 101)
    ]
)

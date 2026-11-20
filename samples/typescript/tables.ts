// An array of records with matching fields appears as a table.
const readings = [
  { city: "Lagos", temperatureC: 29 },
  { city: "Nairobi", temperatureC: 23 },
  { city: "Accra", temperatureC: 28 },
];

readings.map(({ city, temperatureC }) => ({
  city,
  temperatureC,
  temperatureF: Math.round(temperatureC * 9 / 5 + 32),
}));

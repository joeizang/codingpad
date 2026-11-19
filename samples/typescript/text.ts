const json = '{"event":"release","when":"2026-11-01T09:30:00Z"}';
const { event, when } = JSON.parse(json);
const date = new Date(when);

({
  label: `${event.toUpperCase()} · ${date.toISOString()}`,
  weekday: new Intl.DateTimeFormat("en", {
    weekday: "long",
    timeZone: "UTC",
  }).format(date),
});

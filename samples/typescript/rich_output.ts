import { dump, html, live } from "codingpad";

const status = live("Starting");
dump(status);
for (let step = 1; step <= 3; step++) {
  await Bun.sleep(150);
  status.value = `Step ${step} of 3`;
}

status.value = "Complete";
dump(html("<strong>All three steps completed.</strong>"));

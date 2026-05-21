import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import yaml from "js-yaml";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default function () {
  const locale = process.env.SITE_LOCALE || "en";
  const file = path.join(__dirname, "..", "_content", `${locale}.yaml`);
  return yaml.load(fs.readFileSync(file, "utf8"));
}

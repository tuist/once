import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import yaml from "js-yaml";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default function () {
  const locale = process.env.SITE_LOCALE || "en";
  const file = path.join(__dirname, "..", "_content", `${locale}.yaml`);
  const content = yaml.load(fs.readFileSync(file, "utf8"));
  return {
    locale,
    currentYear: new Date().getFullYear(),
    url: "https://once.tuist.dev",
    github: "https://github.com/tuist/once",
    docs: "https://once.tuist.dev",
    name: content.site.name,
    title: content.site.title,
    description: content.site.description,
    ogAlt: content.site.ogAlt,
  };
}

import fs from "node:fs/promises";
import { readFileSync } from "node:fs";
import path from "node:path";

import rssPlugin from "@11ty/eleventy-plugin-rss";
import yaml from "js-yaml";

import { generateSocialImages } from "./scripts/generate-social-images.mjs";

export default function (eleventyConfig) {
  eleventyConfig.addPlugin(rssPlugin);
  eleventyConfig.addDataExtension("yaml,yml", (contents) => yaml.load(contents));

  eleventyConfig.addPassthroughCopy({ "src/public": "/" });
  eleventyConfig.addPassthroughCopy({ "src/assets": "assets" });

  eleventyConfig.addWatchTarget("./src/assets/");
  eleventyConfig.addWatchTarget("./src/blog/posts/");

  eleventyConfig.on("eleventy.before", async () => {
    await generateSocialImages();
  });

  eleventyConfig.on("eleventy.after", async ({ dir, results }) => {
    const postsRoot = path.resolve(dir.input, "blog", "posts");
    for (const result of results) {
      const inputPath = path.resolve(result.inputPath);
      if (!inputPath.startsWith(postsRoot)) continue;
      const raw = await fs.readFile(inputPath, "utf8");
      const outDir = path.dirname(result.outputPath);
      await fs.writeFile(path.join(outDir, "index.md"), raw, "utf8");
    }
  });

  eleventyConfig.addCollection("posts", (collections) =>
    collections
      .getFilteredByGlob("./src/blog/posts/*.md")
      .sort((a, b) => b.date - a.date),
  );

  eleventyConfig.addFilter("readableDate", (date) =>
    new Date(date).toLocaleDateString("en-US", {
      year: "numeric",
      month: "long",
      day: "numeric",
    }),
  );

  eleventyConfig.addFilter("isoDate", (date) =>
    new Date(date).toISOString(),
  );

  eleventyConfig.addFilter("mdAlternate", (url) => {
    if (!url || typeof url !== "string") return url;
    return url.endsWith("/") ? `${url}index.md` : `${url}.md`;
  });

  eleventyConfig.addFilter("inlineMd", (value) => {
    if (typeof value !== "string") return value;
    return value.replace(/`([^`]+)`/g, "<code>$1</code>");
  });

  eleventyConfig.addFilter("stripMd", (value) => {
    if (typeof value !== "string") return value;
    return value.replace(/`([^`]+)`/g, "$1").replace(/<[^>]+>/g, "");
  });

  eleventyConfig.addFilter("rawMarkdown", (postOrPath) => {
    const inputPath =
      typeof postOrPath === "string"
        ? postOrPath
        : postOrPath?.inputPath ?? postOrPath?.page?.inputPath;
    if (!inputPath) return "";
    const raw = readFileSync(inputPath, "utf8");
    const match = raw.match(/^---[\s\S]*?\n---\n([\s\S]*)$/);
    return match ? match[1].trimStart() : raw;
  });

  eleventyConfig.addFilter("tomlHighlight", (text) => {
    if (typeof text !== "string") return text;
    const escape = (s) =>
      s
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;");
    return escape(text)
      .replace(
        /"([^"\n]*)"/g,
        '<span data-part="value">"$1"</span>',
      )
      .replace(
        /^(\[\[[^\]\n]+\]\]|\[[^\]\n]+\])$/gm,
        '<span data-part="muted">$1</span>',
      );
  });

  eleventyConfig.addFilter("terminalHtml", (text) => {
    if (typeof text !== "string") return text;
    const valueSpan = (line) =>
      line.replace(/`([^`]+)`/g, '<span data-part="value">$1</span>');
    return text
      .split("\n")
      .map((line) => {
        if (line.startsWith("$ "))
          return `<span data-part="prompt">$</span> ${valueSpan(line.slice(2))}`;
        if (line.startsWith("→ "))
          return `<span data-part="muted">→</span> ${valueSpan(line.slice(2))}`;
        return valueSpan(line);
      })
      .join("\n");
  });

  return {
    dir: {
      input: "src",
      includes: "_includes",
      data: "_data",
      output: "_site",
    },
    templateFormats: ["njk", "md", "html"],
    markdownTemplateEngine: "njk",
    htmlTemplateEngine: "njk",
  };
}

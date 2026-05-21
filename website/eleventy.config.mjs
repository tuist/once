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

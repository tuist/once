import rssPlugin from "@11ty/eleventy-plugin-rss";

import { generateSocialImages } from "./scripts/generate-social-images.mjs";

export default function (eleventyConfig) {
  eleventyConfig.addPlugin(rssPlugin);

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

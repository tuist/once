export default {
  layout: "layouts/post.njk",
  tags: ["posts"],
  permalink: "/blog/{{ page.fileSlug }}/",
  eleventyComputed: {
    socialImage: (data) => `${data.site.url}/assets/social/${data.page.fileSlug}.png`,
    socialImageAlt: (data) => `${data.title} · ${data.site.name}`,
  },
};

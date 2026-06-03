import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import matter from "gray-matter";
import sharp from "sharp";
import yaml from "js-yaml";

const WIDTH = 1200;
const HEIGHT = 630;
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const rootDir = path.resolve(__dirname, "..");
const postsDir = path.join(rootDir, "src", "blog", "posts");
const outputDir = path.join(rootDir, "src", "assets", "social");
const logoPath = path.join(rootDir, "src", "public", "logo.png");

function escapeHtml(value) {
  return String(value)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function wrapText(text, maxLineLength, maxLines) {
  const words = String(text).trim().split(/\s+/).filter(Boolean);
  const lines = [];
  let current = "";

  for (const word of words) {
    const next = current ? `${current} ${word}` : word;

    if (next.length <= maxLineLength) {
      current = next;
      continue;
    }

    if (current) {
      lines.push(current);
      current = word;
    } else {
      lines.push(word);
    }

    if (lines.length === maxLines) {
      break;
    }
  }

  if (current && lines.length < maxLines) {
    lines.push(current);
  }

  if (lines.length === maxLines && words.join(" ").length > lines.join(" ").length) {
    lines[maxLines - 1] = lines[maxLines - 1].replace(/[,.!?;:]?$/, "") + "...";
  }

  return lines;
}

function formatDate(value) {
  if (!value) return null;
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleDateString("en-US", {
    year: "numeric",
    month: "long",
    day: "numeric",
    timeZone: "UTC",
  });
}

function renderSvg({ title, date, kicker }) {
  const lines = wrapText(title, 22, 3);
  const fontSize = lines.length >= 3 ? 64 : 78;
  const lineHeight = Math.round(fontSize * 1.12);
  const titleTop = lines.length === 1 ? 332 : lines.length === 2 ? 296 : 264;
  const serifStack = "Petrona, 'Source Serif Pro', 'Georgia', 'Times New Roman', serif";
  const sansStack = "Geist, Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";

  const titleLines = lines
    .map(
      (line, index) =>
        `<tspan x="390" y="${titleTop + index * lineHeight}">${escapeHtml(line)}</tspan>`
    )
    .join("");

  const footerY = 540;
  const dateLine = date
    ? `<text x="390" y="${footerY - 40}" font-family="${sansStack}" font-size="28" font-weight="600" fill="#8c8a82">${escapeHtml(date)}</text>`
    : "";

  return `
<svg width="${WIDTH}" height="${HEIGHT}" viewBox="0 0 ${WIDTH} ${HEIGHT}" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#fdfdfc"/>
      <stop offset="1" stop-color="#f6f0e0"/>
    </linearGradient>
    <radialGradient id="glow" cx="65%" cy="35%" r="60%">
      <stop offset="0" stop-color="#f1d896" stop-opacity="0.55"/>
      <stop offset="0.6" stop-color="#fdfdfc" stop-opacity="0"/>
      <stop offset="1" stop-color="#fdfdfc" stop-opacity="0"/>
    </radialGradient>
  </defs>
  <rect width="${WIDTH}" height="${HEIGHT}" fill="url(#bg)"/>
  <rect width="${WIDTH}" height="${HEIGHT}" fill="url(#glow)"/>
  <path d="M0 120H1200M0 510H1200" stroke="#20201d" stroke-opacity="0.06"/>
  <path d="M90 0V630M1110 0V630" stroke="#20201d" stroke-opacity="0.04"/>
  <text x="390" y="200" font-family="${sansStack}" font-size="30" font-weight="800" letter-spacing="8" fill="#ad7c1f">${escapeHtml(kicker || "ONCE")}</text>
  <text font-family="${serifStack}" font-size="${fontSize}" font-weight="800" fill="#20201d" letter-spacing="-1">${titleLines}</text>
  ${dateLine}
  <text x="390" y="${footerY}" font-family="${sansStack}" font-size="26" font-weight="600" fill="#605d54">once.tuist.dev</text>
</svg>`;
}

async function renderImage(svg, logoBuffer, outputPath) {
  const image = await sharp(Buffer.from(svg))
    .composite([{ input: logoBuffer, left: 130, top: 230 }])
    .png()
    .toBuffer();

  let existing = null;
  try {
    existing = await fs.readFile(outputPath);
  } catch {}

  if (!existing || !existing.equals(image)) {
    await fs.writeFile(outputPath, image);
  }
}

async function renderPostImage(postPath, logoBuffer) {
  const baseName = path.basename(postPath, path.extname(postPath));
  const slug = baseName.replace(/^\d{4}-\d{2}-\d{2}-/, "");
  const source = await fs.readFile(path.join(postsDir, postPath), "utf8");
  const { data } = matter(source);

  if (!data.title) return null;

  const svg = renderSvg({
    title: data.title,
    date: formatDate(data.date),
    kicker: "ONCE · BLOG",
  });

  const outputPath = path.join(outputDir, `${slug}.png`);
  await renderImage(svg, logoBuffer, outputPath);
  return `${slug}.png`;
}

async function loadCopy() {
  const locale = process.env.SITE_LOCALE || "en";
  const file = path.join(rootDir, "src", "_content", `${locale}.yaml`);
  return yaml.load(await fs.readFile(file, "utf8"));
}

async function renderStaticPageImages(logoBuffer, copy) {
  const pages = [
    {
      slug: "default",
      title: copy?.home?.hero?.title || "A polyglot automation kernel for humans and agents.",
      kicker: "ONCE",
    },
    {
      slug: "blog",
      title: copy?.blog?.headline || "Updates from the workshop.",
      kicker: "ONCE · BLOG",
    },
  ];

  const results = [];
  for (const page of pages) {
    const svg = renderSvg(page);
    const outputPath = path.join(outputDir, `${page.slug}.png`);
    await renderImage(svg, logoBuffer, outputPath);
    results.push(`${page.slug}.png`);
  }
  return results;
}

export async function generateSocialImages() {
  await fs.mkdir(outputDir, { recursive: true });

  const logoBuffer = await sharp(logoPath)
    .resize(180, 180, { fit: "contain", background: { r: 0, g: 0, b: 0, alpha: 0 } })
    .png()
    .toBuffer();

  const copy = await loadCopy();
  const expected = new Set();
  for (const name of await renderStaticPageImages(logoBuffer, copy)) {
    expected.add(name);
  }

  const posts = (await fs.readdir(postsDir))
    .filter((file) => file.endsWith(".md"))
    .sort();

  for (const post of posts) {
    const output = await renderPostImage(post, logoBuffer);
    if (output) expected.add(output);
  }

  const generated = (await fs.readdir(outputDir)).filter((file) => file.endsWith(".png"));
  await Promise.all(
    generated
      .filter((file) => !expected.has(file))
      .map((file) => fs.rm(path.join(outputDir, file)))
  );
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await generateSocialImages();
}

/** @type {import('next').NextConfig} */
const nextConfig = {
  // Static export: the site builds to plain HTML/CSS/JS in `out/` and can be
  // served from any static host (GitHub Pages, S3, nginx, ...).
  output: 'export',
  images: { unoptimized: true },
};

export default nextConfig;

# rakkio brand

The lowercase wordmark uses outlined Inter SemiBold glyphs for `rakkio`. The standalone icon and adaptive favicon use the same typeface's lowercase `r`.

- `src/components/BrandLogo.vue` renders the wordmark inline with `currentColor`, a single accessible name, and no external font request or duplicate SVG IDs.
- `icon.svg` provides a standalone 96 x 96 icon for provider-specific exports.
- `/favicon.svg` uses the same icon geometry and adapts to the browser's light/dark preference.
- `/favicon.png` is a 96 x 96 raster fallback of `icon.svg`, navy on opaque white for predictable contrast on external surfaces.
- `/favicon.ico` contains matching 16, 32, 48 and 96 pixel PNG frames for conventional favicon discovery. All three URLs are declared in the homepage and excluded from Vercel's SPA rewrite, so `/favicon.ico` is an image rather than HTML.

Raster exports preserve the standalone SVG geometry: render `icon.svg` on a square white canvas with `color: #0f172a` at each output size, then package the four PNG frames in an ICO directory. No font download or new artwork is needed. `branding.spec.ts` checks declarations, response MIME types, PNG dimensions/decoding, ICO directory/frame identity and deployment rewrite exclusions.

Google may continue displaying its previously cached favicon after deployment. Request homepage recrawling through Search Console's URL Inspection tool; updates can take days to weeks and aren't guaranteed. See [Google's favicon guidance](https://developers.google.com/search/docs/appearance/favicon-in-search). This is separate from the application logo configured in Google Auth Platform.

The artwork is derived from Inter by The Inter Project Authors (https://github.com/rsms/inter), licensed under SIL OFL 1.1. The complete license is included in `Inter-OFL.txt`. Geometry and accessible naming are covered by `BrandLogo.test.ts`.

The former paragraph-p artwork is retained in Git history, not shipped as the rakkio logo.

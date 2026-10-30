// Ícones da PWA (issue #203) a partir do desenho de src-tauri/icons/icon.svg:
// quadrado arredondado escuro com a mira âmbar (quatro traços e um anel).
// Sem dependências: o desenho é rasterizado por distância (SDF) com
// superamostragem 4×4 e gravado em PNG com o zlib do Node. Reproduzível:
//
//   npm run pwa:icons        (em ui/; grava em pwa/icons/)
//
// Saída: icon-192.png e icon-512.png (purpose "any", cantos arredondados),
// maskable-512.png (fundo cheio, mira dentro da zona segura de 80%),
// apple-touch-icon.png (180 px, fundo cheio: o iOS arredonda) e icon.svg.
import { deflateSync } from "node:zlib";
import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const out = join(root, "pwa", "icons");

const BG = [0x0f, 0x11, 0x15];
const FG = [0xf2, 0xa3, 0x3a];
const STROKE = 1; // meia largura do traço (2 no SVG), em unidades de 24
const SEGMENTS = [
  [12, 3.5, 12, 8],
  [12, 20.5, 12, 16],
  [3.5, 12, 8, 12],
  [20.5, 12, 16, 12],
];

function segDist(px, py, [ax, ay, bx, by]) {
  const dx = bx - ax;
  const dy = by - ay;
  const t = Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)));
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
}

/** Dentro do quadrado arredondado (0..24, raio r)? */
function inRoundRect(x, y, r) {
  if (r === 0) return x >= 0 && x <= 24 && y >= 0 && y <= 24;
  const cx = Math.max(r, Math.min(24 - r, x));
  const cy = Math.max(r, Math.min(24 - r, y));
  return Math.hypot(x - cx, y - cy) <= r;
}

function inGlyph(x, y) {
  if (Math.abs(Math.hypot(x - 12, y - 12) - 4) <= STROKE) return true;
  return SEGMENTS.some((s) => segDist(x, y, s) <= STROKE);
}

/** RGBA de `size`×`size`; `radius` do fundo e `scale` da mira em volta do centro. */
function render(size, radius, scale) {
  const px = Buffer.alloc(size * size * 4);
  const N = 4;
  for (let j = 0; j < size; j++) {
    for (let i = 0; i < size; i++) {
      let bg = 0;
      let fg = 0;
      for (let sj = 0; sj < N; sj++) {
        for (let si = 0; si < N; si++) {
          const x = ((i + (si + 0.5) / N) / size) * 24;
          const y = ((j + (sj + 0.5) / N) / size) * 24;
          if (!inRoundRect(x, y, radius)) continue;
          bg++;
          if (inGlyph(12 + (x - 12) / scale, 12 + (y - 12) / scale)) fg++;
        }
      }
      const k = (j * size + i) * 4;
      const a = bg / (N * N);
      const f = bg ? fg / bg : 0;
      for (let c = 0; c < 3; c++) px[k + c] = Math.round(BG[c] * (1 - f) + FG[c] * f);
      px[k + 3] = Math.round(a * 255);
    }
  }
  return px;
}

const CRC = new Int32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c;
});

function crc32(buf) {
  let c = -1;
  for (const b of buf) c = CRC[(c ^ b) & 0xff] ^ (c >>> 8);
  return (c ^ -1) >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

function png(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr.set([8, 6, 0, 0, 0], 8);
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let j = 0; j < size; j++) {
    rgba.copy(raw, j * (size * 4 + 1) + 1, j * size * 4, (j + 1) * size * 4);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

mkdirSync(out, { recursive: true });
const icons = [
  ["icon-192.png", 192, 5, 1],
  ["icon-512.png", 512, 5, 1],
  ["maskable-512.png", 512, 0, 0.8],
  ["apple-touch-icon.png", 180, 0, 0.9],
];
for (const [name, size, radius, scale] of icons) {
  writeFileSync(join(out, name), png(size, render(size, radius, scale)));
  console.log(`pwa/icons/${name}`);
}
copyFileSync(join(root, "src-tauri", "icons", "icon.svg"), join(out, "icon.svg"));
console.log("pwa/icons/icon.svg");

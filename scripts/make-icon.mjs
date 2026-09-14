// 生成一张 512x512 的源图标：烟熏玻璃面板 + 被光打亮的顶边 + 黄铜拉手
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const S = 512, M = 26, R = 108;

const crc32 = (buf) => {
  let crc = 0xffffffff;
  for (const byte of buf) {
    crc ^= byte;
    for (let k = 0; k < 8; k++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return (crc ^ 0xffffffff) >>> 0;
};

const chunk = (type, data) => {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, crc]);
};

// 圆角矩形内部判定
const inRR = (x, y, x0, y0, x1, y1, r) => {
  if (x < x0 || x > x1 || y < y0 || y > y1) return false;
  const cx = Math.min(Math.max(x, x0 + r), x1 - r);
  const cy = Math.min(Math.max(y, y0 + r), y1 - r);
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r;
};

const mix = (a, b, t) => a.map((v, i) => v + (b[i] - v) * t);
const smoothstep = (e0, e1, x) => {
  const t = Math.min(1, Math.max(0, (x - e0) / (e1 - e0)));
  return t * t * (3 - 2 * t);
};

function sample(x, y) {
  if (!inRR(x, y, M, M, S - M, S - M, R)) return [0, 0, 0, 0];

  const t = (y - M) / (S - 2 * M);
  let col = mix([0x2b, 0x2c, 0x33], [0x0b, 0x0c, 0x0f], t);

  // 顶边那条被光打亮的线
  col = mix(col, [255, 255, 255], (1 - smoothstep(0, 30, y - M)) * 0.5);

  // 黄铜拉手
  const bw = 176, bh = 18, by = S * 0.58;
  if (inRR(x, y, S / 2 - bw / 2, by, S / 2 + bw / 2, by + bh, bh / 2)) {
    col = mix(col, [0xd8, 0xa6, 0x57], 0.94);
  }
  return [col[0], col[1], col[2], 255];
}

const raw = Buffer.alloc(S * (S * 4 + 1));
let p = 0;
for (let y = 0; y < S; y++) {
  raw[p++] = 0; // filter: none
  for (let x = 0; x < S; x++) {
    let r = 0, g = 0, b = 0, a = 0;
    for (let sy = 0; sy < 3; sy++) {
      for (let sx = 0; sx < 3; sx++) {
        const c = sample(x + (sx + 0.5) / 3, y + (sy + 0.5) / 3);
        r += c[0]; g += c[1]; b += c[2]; a += c[3];
      }
    }
    raw[p++] = Math.round(r / 9);
    raw[p++] = Math.round(g / 9);
    raw[p++] = Math.round(b / 9);
    raw[p++] = Math.round(a / 9);
  }
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8;  // bit depth
ihdr[9] = 6;  // RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw, { level: 9 })),
  chunk("IEND", Buffer.alloc(0)),
]);

writeFileSync(process.argv[2], png);
console.log("wrote", process.argv[2], png.length, "bytes");

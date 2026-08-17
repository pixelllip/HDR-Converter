// 手动拼接多尺寸 ICO（支持 PNG-in-ICO，Windows Vista+ 原生支持）
// ICO 文件结构：
//   - ICONDIR (6 bytes)
//   - ICONDIRENTRY × N (16 bytes each)
//   - PNG data (按尺寸)

const fs = require('fs');
const path = require('path');
const sharp = require('sharp');

const assetsDir = path.join(__dirname, '..', 'assets');
const sizes = [16, 24, 32, 48, 64, 128, 256];

async function main() {
  const pngEntries = [];
  for (const size of sizes) {
    // 重新合成每个尺寸的 PNG（统一从 512 大图标降采样，保证锐利）
    const buf = await sharp(path.join(assetsDir, 'logo.svg'))
      .resize(size, size)
      .png()
      .toBuffer();
    pngEntries.push({ size, buf });
    console.log(`✓ 生成 ${size}×${size} PNG（${buf.length} bytes）`);
  }

  // 构造 ICO
  // ICONDIR: reserved(2)=0, type(2)=1, count(2)=N
  const dirSize = 6 + pngEntries.length * 16;
  let offset = dirSize;
  const dir = Buffer.alloc(6);
  dir.writeUInt16LE(0, 0);
  dir.writeUInt16LE(1, 2);
  dir.writeUInt16LE(pngEntries.length, 4);

  const entries = [];
  for (const e of pngEntries) {
    const entry = Buffer.alloc(16);
    // 宽/高：0 表示 256
    entry.writeUInt8(e.size === 256 ? 0 : e.size, 0);
    entry.writeUInt8(e.size === 256 ? 0 : e.size, 1);
    entry.writeUInt8(0, 2);   // 调色板
    entry.writeUInt8(0, 3);   // 保留
    entry.writeUInt16LE(1, 4);  // 平面数
    entry.writeUInt16LE(32, 6); // 位深
    entry.writeUInt32LE(e.buf.length, 8);  // 数据大小
    entry.writeUInt32LE(offset, 12);       // 数据偏移
    entries.push(entry);
    offset += e.buf.length;
  }

  const outPath = path.join(assetsDir, 'logo.ico');
  const ico = Buffer.concat([dir, ...entries, ...pngEntries.map(e => e.buf)]);
  fs.writeFileSync(outPath, ico);
  console.log(`\n✅ 写出 ${outPath}（${(ico.length / 1024).toFixed(1)} KB，${sizes.length} 个尺寸）`);
}

main().catch(e => { console.error(e); process.exit(1); });

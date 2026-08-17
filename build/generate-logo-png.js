// 生成多尺寸 PNG logo
const sharp = require('sharp');
const fs = require('fs');
const path = require('path');

const assetsDir = path.join(__dirname, '..', 'assets');

// 各尺寸生成任务
const tasks = [
  { src: 'logo.svg',        name: 'logo-512.png',        size: 512 },
  { src: 'logo.svg',        name: 'logo-256.png',        size: 256 },
  { src: 'logo.svg',        name: 'logo-128.png',        size: 128 },
  { src: 'logo.svg',        name: 'logo-64.png',         size: 64  },
  { src: 'logo-icon.svg',   name: 'logo-icon-512.png',   size: 512 },
  { src: 'logo-icon.svg',   name: 'logo-icon-256.png',   size: 256 },
  { src: 'logo-icon.svg',   name: 'logo-icon-128.png',   size: 128 },
  { src: 'logo-icon.svg',   name: 'logo-icon-64.png',    size: 64  },
  { src: 'logo-icon.svg',   name: 'logo-icon-32.png',    size: 32  },
  { src: 'logo-icon.svg',   name: 'logo-icon-16.png',    size: 16  },
  { src: 'logo-horizontal.svg', name: 'logo-horizontal-800.png', size: 760, h: 200 },
  { src: 'logo-horizontal.svg', name: 'logo-horizontal-400.png', size: 380, h: 100 },
];

(async () => {
  for (const task of tasks) {
    const srcPath = path.join(assetsDir, task.src);
    const dstPath = path.join(assetsDir, task.name);
    try {
      const width = task.size;
      const height = task.h || task.size;
      await sharp(srcPath)
        .resize(width, height, { fit: 'contain', background: { r:0,g:0,b:0,alpha:0 } })
        .png({ compressionLevel: 9 })
        .toFile(dstPath);
      console.log(`✓ ${task.name} (${task.size}×${task.h || task.size})`);
    } catch (e) {
      console.error(`✗ ${task.name}: ${e.message}`);
    }
  }
  console.log('\n全部生成完毕！');
})();

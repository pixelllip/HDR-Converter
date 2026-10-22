// 类型检查门禁：跑 `tsc --noEmit` 校验 types/ 下的 IPC 契约声明，
// 以及所有带 `// @ts-check` 的 JS 文件（目前是 preload.js —— 契约的实现方）。
//
// 用法：node tests/check_types.js   （或 npm run typecheck）
// 退出码：0 = 通过；1 = 有类型错误或环境缺 typescript。
//
// 说明：tsconfig.json 里 checkJs 为 false，采用「逐文件 opt-in」模型 ——
// 需要校验的 .js 在首行加 `// @ts-check`。既有代码的待修清单见 docs/TS_MIGRATION_zh.md。
'use strict'
const path = require('path')
const fs = require('fs')
const { spawnSync } = require('child_process')

const root = path.join(__dirname, '..')
const tscJs = path.join(root, 'node_modules', 'typescript', 'bin', 'tsc')

if (!fs.existsSync(tscJs)) {
  console.error('❌ 未找到 typescript。请先安装开发依赖：npm install')
  process.exit(1)
}

// 用当前 node 直接跑 tsc 的入口 js（不依赖 npx / PATH，Windows 下也稳）
const r = spawnSync(process.execPath, [tscJs, '--noEmit'], {
  cwd: root,
  encoding: 'utf8',
  windowsHide: true
})

if (r.error) {
  console.error('❌ 无法启动 tsc: ' + r.error.message)
  process.exit(1)
}

const out = String(r.stdout || '') + String(r.stderr || '')
const errs = out.split(/\r?\n/).filter((l) => /error TS\d+/.test(l))

if (r.status === 0 && errs.length === 0) {
  console.log('✅ 类型检查通过（types/ipc.d.ts 契约 + preload.js 实现）')
  process.exit(0)
}

console.error('❌ 类型检查失败：' + errs.length + ' 个错误')
console.error(out.trim())
process.exit(1)

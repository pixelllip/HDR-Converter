// 前端完整性检查：提取 views/*.html 的内联 <script> 做语法解析，
// 并交叉验证 JS 中 getElementById('...') 引用的 id 是否都存在于对应 HTML。
// 用法：node tests/check_inline_syntax.js
'use strict'
const fs = require('fs')
const path = require('path')

const files = ['views/home.html', 'views/image.html', 'views/video.html']
const reScript = /<script(?![^>]*src=)[^>]*>([\s\S]*?)<\/script>/g
const reId = /\bid="([^"]+)"/g
const reGetById = /(?:getElementById|\$id|setText|bind)\('([^']+)'\)/g
let total = 0
let failed = false

for (const f of files) {
  const abs = path.join(__dirname, '..', f)
  const html = fs.readFileSync(abs, 'utf8')

  // 1) JS 语法解析
  let m, i = 0
  const scripts = []
  while ((m = reScript.exec(html)) !== null) {
    i++
    total++
    scripts.push(m[1])
    try {
      new Function(m[1]) // 只解析不执行
    } catch (e) {
      console.error(`SYNTAX FAIL ${f} script#${i}: ${e.message}`)
      failed = true
    }
  }
  console.log(`OK ${f}: ${i} inline script(s) parsed`)

  // 2) id 交叉验证：JS 引用的 id 必须存在于 HTML
  const defined = new Set()
  let mm
  while ((mm = reId.exec(html)) !== null) defined.add(mm[1])
  const referenced = new Set()
  for (const s of scripts) {
    let r
    while ((r = reGetById.exec(s)) !== null) referenced.add(r[1])
  }
  const missing = [...referenced].filter((id) => !defined.has(id)).sort()
  if (missing.length) {
    console.error(`ID MISSING in ${f}: ${missing.join(', ')}`)
    failed = true
  } else {
    console.log(`OK ${f}: all ${referenced.size} referenced id(s) exist in HTML`)
  }
}

console.log(failed ? 'CHECK FAILED' : `All ${total} inline scripts OK.`)
process.exit(failed ? 1 : 0)

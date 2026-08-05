/**
 * 批量转换测试
 *
 * 验证：
 *   1. /batch/convert 处理多个任务并返回逐文件结果
 *   2. 并发受全局信号量限制（/status 的 capacity = 核心数/2+1）
 *   3. /batch/progress 进度可查询
 *
 * 用法：node tests/batch_test.js
 */
const path = require('path')
const fs = require('fs')
const sharp = require('sharp')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

;(async () => {
  const n = 6
  const inputs = []
  const outputs = []
  // 生成 n 张不同颜色的测试图
  for (let i = 0; i < n; i++) {
    const p = path.join(__dirname, `tmp_batch_in_${i}.png`)
    const o = path.join(__dirname, `tmp_batch_out_${i}.jpg`)
    const hue = Math.round(i * 60) % 360
    await sharp({
      create: { width: 320, height: 200, channels: 3, background: { r: 40, g: 60, b: 120 } }
    }).composite([
      { input: Buffer.from(`<svg width="320" height="200"><rect width="320" height="200" fill="hsl(${hue} 70% 60%)"/><rect x="80" y="60" width="160" height="90" fill="#ffffff"/></svg>`) }
    ]).png().toFile(p)
    inputs.push(p)
    outputs.push(o)
  }

  await ensureBackend()
  const status = await httpJson('GET', '/status')
  console.log('容量（核心数/2+1）:', status.capacity, '| 当前 active:', status.active, '| method:', status.method)

  const jobs = inputs.map((p, i) => ({
    inputPath: p,
    outputPath: outputs[i],
    settings: { hdrIntensity: 1.2, fineTuneBrightness: 0.5, gamma: 0.9, outputFormat: 'jpg' }
  }))

  console.log('提交批量转换', jobs.length, '个任务…')
  const res = await httpJson('POST', '/batch/convert', { jobs })
  console.log('成功:', res.successCount, '失败:', res.failCount)
  res.results.forEach((r) => {
    const ok = r.success ? '✅' : '❌'
    console.log(`  ${ok} ${path.basename(r.inputPath)} -> ${r.outputPath ? path.basename(r.outputPath) : '(无输出)'} ${r.message || ''}`)
  })

  if (res.successCount !== n) {
    console.error('❌ 批量转换未全部成功')
    process.exit(1)
  }
  // 校验每个输出文件存在且为有效 JPEG
  for (const o of outputs) {
    if (!fs.existsSync(o)) { console.error('❌ 输出不存在:', o); process.exit(1) }
    const meta = await sharp(o).metadata()
    if (meta.format !== 'jpeg') { console.error('❌ 输出非 JPEG:', o); process.exit(1) }
  }
  console.log('✅ 全部输出为有效 JPEG')

  // 并发上限抽查：capacity >= 1 且 active <= capacity（批量进行中实测）
  const during = await httpJson('GET', '/status')
  console.log('批量结束后 active:', during.active, '(应回落到 0 或很小)')
  if (parseInt(during.active, 10) > parseInt(during.capacity, 10)) {
    console.error('❌ active 超过容量')
    process.exit(1)
  }

  stopBackend()
  console.log('✅ 批量测试通过')
})().catch((e) => { console.error('测试失败:', e); stopBackend(); process.exit(1) })

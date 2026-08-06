/**
 * 临时验证：批量逐项状态 + 取消指定图片
 */
const path = require('path')
const os = require('os')
const fs = require('fs')
const sharp = require('sharp')
const { ensureBackend, stopBackend, httpJson } = require('./backend_test_util')

async function main() {
    const n = 20
    const inputs = []
    const outputs = []
    for (let i = 0; i < n; i++) {
        const p = path.join(os.tmpdir(), `hdr_cancel_in_${i}.png`)
        const o = path.join(os.tmpdir(), `hdr_cancel_out_${i}.jpg`)
        await sharp({ create: { width: 1600, height: 1200, channels: 3, background: { r: 40, g: 60, b: 120 } } })
            .composite([
                { input: Buffer.from(`<svg width="1600" height="1200"><rect width="1600" height="1200" fill="hsl(${Math.round(i * 60) % 360} 70% 60%)"/><rect x="300" y="250" width="900" height="600" fill="#ffffff"/></svg>`) }
            ]).png().toFile(p)
        inputs.push(p); outputs.push(o)
    }

    await ensureBackend()

    // 提交批量（不 await，让请求在后台跑）
    const jobs = inputs.map((p, i) => ({
        inputPath: p,
        outputPath: outputs[i],
        settings: { hdrIntensity: 1.2, fineTuneBrightness: 0.5, gamma: 0.9, outputFormat: 'jpg' }
    }))
    const batchPromise = httpJson('POST', '/batch/convert', { jobs })

    // 等 150ms 后取消最后一个任务（1600x1200 转换较慢，此时它大概率还在排队）
    await new Promise((r) => setTimeout(r, 150))
    const cancelRes = await httpJson('POST', '/batch/cancel', { inputPaths: [inputs[n - 1]] })
    console.log('/batch/cancel 返回:', JSON.stringify(cancelRes))

    // 查询进行中的逐项状态
    const prog = await httpJson('GET', '/batch/progress')
    console.log('/batch/progress:', JSON.stringify({
        total: prog.total, done: prog.done, failed: prog.failed, running: prog.running,
        current: prog.current, message: prog.message,
        statuses: prog.statuses
    }))
    console.log('statuses 键数量:', Object.keys(prog.statuses || {}).length)

    const res = await batchPromise
    const last = res.results[n - 1]
    console.log('最后一个任务结果:', JSON.stringify(last))

    // 校验最后一个任务被取消
    const ok = last && last.message === '已取消' && last.success === false
    const othersOk = res.results.slice(0, n - 1).every((r) => r.success === true)
    console.log('取消成功:', ok ? '✅' : '❌', '| 其余任务全部成功:', othersOk ? '✅' : '❌')

    // 清理
    inputs.forEach((p) => { try { fs.unlinkSync(p) } catch (e) { } })
    outputs.forEach((o) => { try { fs.unlinkSync(o) } catch (e) { } })
    stopBackend()
    process.exit(ok && othersOk ? 0 : 1)
}

main().catch((e) => { console.error('验证失败:', e); stopBackend(); process.exit(1) })

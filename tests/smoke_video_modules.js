// 冒烟测试：确认主进程模块 require 链可加载（main.js 在纯 node 下因 electron 缺失会报错，
// 这里只验证 video_converter / mp4_hdr 两个新模块语法与导出正常）
const path = require('path')
const root = 'c:/Users/Administrator/Documents/Java/hdr_electron'

const vc = require(path.join(root, 'video_converter.js'))
const mh = require(path.join(root, 'mp4_hdr.js'))

const checks = [
    ['video_converter', ['probeVideo', 'convertVideoDirect', 'convertVideoFrames', 'extractFirstFrame', 'cancelAllFFmpeg']],
    ['mp4_hdr', ['injectHdrBoxes', 'DEFAULT_MASTERING']]
]
let ok = true
for (const [mod, fns] of checks) {
    const obj = mod === 'video_converter' ? vc : mh
    for (const fn of fns) {
        const has = typeof obj[fn] === 'function' || (mod === 'mp4_hdr' && fn === 'DEFAULT_MASTERING' && obj[fn])
        if (!has) { ok = false; console.log('❌ 缺少导出:', mod + '.' + fn) }
    }
}
console.log(ok ? '✅ 模块 require 链正常，所有导出齐全' : '❌ 有缺失')
process.exit(ok ? 0 : 1)

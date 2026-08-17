/**
 * MP4 HDR 元数据盒注入（mdcv / clli）
 *
 * Chromium 的 MP4 解析器从容器里的 `colr`(nclx) / `mdcv`(Mastering Display
 * Color Volume) / `clli`(Content Light Level) 三个盒读取 HDR 元数据。
 * ffmpeg/libx265 只会写 `colr` + 码流 SEI，**不会**写 `mdcv`/`clli` 容器盒，
 * 导致 Chromium 在 demuxer 层读不到 HDR 元数据。本模块在编码后把这两个盒
 * 注入视频采样描述（stsd → hvc1/hev1）内，并向上传播所有祖先盒的大小。
 *
 * 布局前提：ffmpeg 默认 `moov` 位于文件末尾（ftyp/free/mdat/moov），注入
 * moov 内不移动 mdat → stco/co64 块偏移不变。若检测到 moov 在 mdat 之前，
 * 会自动把 stco/co64 的块偏移整体加上插入字节数。
 */
const fs = require('fs')

// 默认 P3 主色 mastering display（与 -x265-params master-display 完全一致）
const DEFAULT_MASTERING = {
    gx: 13250, gy: 34500,
    bx: 7500, by: 3000,
    rx: 34000, ry: 16000,
    wx: 15635, wy: 16450,
    maxLum: 10000000, // 1000 cd/m²（单位 0.0001）
    minLum: 1         // 0.0001 cd/m²
}

function readBoxSize(buf, off) {
    const s = buf.readUInt32BE(off)
    if (s === 1) return Number(buf.readBigUInt64BE(off + 8))
    return s
}

function boxHeaderSize(buf, off) {
    return buf.readUInt32BE(off) === 1 ? 16 : 8
}

function addBoxSize(buf, off, delta) {
    const s = buf.readUInt32BE(off)
    if (s === 1) {
        buf.writeBigUInt64BE(buf.readBigUInt64BE(off + 8) + BigInt(delta), off + 8)
    } else if (s !== 0) {
        buf.writeUInt32BE(s + delta, off)
    }
}

/** 构造 mdcv 盒（32 字节：type + 24 载荷） */
function buildMdcv(m) {
    const b = Buffer.alloc(32)
    b.writeUInt32BE(32, 0)
    b.write('mdcv', 4, 'latin1')
    b.writeUInt16BE(m.gx, 8); b.writeUInt16BE(m.gy, 10)
    b.writeUInt16BE(m.bx, 12); b.writeUInt16BE(m.by, 14)
    b.writeUInt16BE(m.rx, 16); b.writeUInt16BE(m.ry, 18)
    b.writeUInt16BE(m.wx, 20); b.writeUInt16BE(m.wy, 22)
    b.writeUInt32BE(m.maxLum, 24)
    b.writeUInt32BE(m.minLum, 28)
    return b
}

/** 构造 clli 盒（12 字节） */
function buildClli(maxCll, maxFall) {
    const b = Buffer.alloc(12)
    b.writeUInt32BE(12, 0)
    b.write('clli', 4, 'latin1')
    b.writeUInt16BE(maxCll, 8)
    b.writeUInt16BE(maxFall, 10)
    return b
}

/**
 * 在视频采样描述中定位（moov→trak→mdia→minf→stbl→stsd→hvc1/hev1）
 * 返回 { chain: [各祖先盒 header 偏移...], entryHeader: 采样条目 header 偏移, insertOff: 插入点 }
 */
function locateInsertion(buf, moovStart, moovEnd) {
    const CONTAINERS = ['moov', 'trak', 'mdia', 'minf', 'stbl', 'stsd']
    const chain = []
    function walk(start, end, isStsd) {
        let off = start
        while (off + 8 <= end) {
            const size = readBoxSize(buf, off)
            const type = buf.toString('latin1', off + 4, off + 8)
            const dStart = off + boxHeaderSize(buf, off)
            const dEnd = off + size
            if (isStsd) {
                // stsd 的直接子盒 = 采样条目（视频为 hvc1/hev1/avc1）
                if (['hvc1', 'hev1', 'avc1'].includes(type)) {
                    // 视觉采样条目有 78 字节固定头（reserved/data_ref/w/h/分辨率/compressorname…），
                    // 子盒（hvcC/colr/mdcv/clli）从 dataStart+78 开始
                    let anchor = null
                    let e = dStart + 78
                    while (e + 8 <= dEnd) {
                        const esz = readBoxSize(buf, e)
                        const etype = buf.toString('latin1', e + 4, e + 8)
                        if (etype === 'colr') anchor = e
                        if (anchor === null && (etype === 'hvcC' || etype === 'avcC')) anchor = e
                        if (esz === 0) break
                        e += esz
                    }
                    if (anchor === null) return null
                    return { chain: chain.concat(off), insertOff: anchor + readBoxSize(buf, anchor) }
                }
            } else if (CONTAINERS.includes(type)) {
                chain.push(off)
                // stsd / dref 是 fullbox：version/flags(4B) + entry_count(4B)，子盒从 +8 开始
                const childStart = (type === 'stsd' || type === 'dref') ? dStart + 8 : dStart
                const r = walk(childStart, dEnd, type === 'stsd')
                if (r) return r
                chain.pop()
            }
            off = dEnd
            if (size === 0) break
        }
        return null
    }
    return walk(moovStart, moovEnd, false)
}

/** 递归查找所有 stco / co64 盒并调整块偏移（moov 在 mdat 之前时使用） */
function adjustChunkOffsets(buf, moovStart, moovEnd, delta) {
    function walk(start, end) {
        let off = start
        while (off + 8 <= end) {
            const size = readBoxSize(buf, off)
            const type = buf.toString('latin1', off + 4, off + 8)
            const dStart = off + boxHeaderSize(buf, off)
            const dEnd = off + size
            if (type === 'stco' || type === 'co64') {
                // stco/co64 是 fullbox：version/flags(4B) + entry_count(4B) + entries
                const count = buf.readUInt32BE(dStart + 4)
                let p = dStart + 8
                for (let i = 0; i < count; i++) {
                    if (type === 'stco') {
                        buf.writeUInt32BE(buf.readUInt32BE(p) + delta, p)
                        p += 4
                    } else {
                        buf.writeBigUInt64BE(buf.readBigUInt64BE(p) + BigInt(delta), p)
                        p += 8
                    }
                }
            } else if (['moov', 'trak', 'mdia', 'minf', 'stbl'].includes(type)) {
                walk(dStart, dEnd)
            }
            off = dEnd
            if (size === 0) break
        }
    }
    walk(moovStart, moovEnd)
}

/**
 * 向 MP4 文件注入 mdcv / clli 盒（就地改写）
 * @param mp4Path MP4 文件路径
 * @param opts { maxCll, maxFall, mastering }
 * @returns { inserted, maxCll, maxFall }
 */
function injectHdrBoxes(mp4Path, opts = {}) {
    const maxCll = opts.maxCll != null ? opts.maxCll : 800
    const maxFall = opts.maxFall != null ? opts.maxFall : 400
    const mastering = opts.mastering || DEFAULT_MASTERING

    const buf = fs.readFileSync(mp4Path)
    if (buf.length < 16) throw new Error('MP4 文件过小')

    // 顶层盒
    let moov = null
    let mdat = null
    let off = 0
    while (off + 8 <= buf.length) {
        const size = readBoxSize(buf, off)
        const type = buf.toString('latin1', off + 4, off + 8)
        if (type === 'moov') moov = { off, size }
        if (type === 'mdat') mdat = { off, size }
        if (size === 0) break
        off += size
    }
    if (!moov) throw new Error('MP4 中没有 moov 盒')

    const loc = locateInsertion(buf, moov.off, moov.off + moov.size)
    if (!loc) throw new Error('找不到视频采样条目（stsd→hvc1/hev1），无法注入 HDR 盒')

    const insert = Buffer.concat([buildMdcv(mastering), buildClli(maxCll, maxFall)])

    // 从最内到最外逐层插入（先做最内层位置，因为 insertOff 是最内层锚点，
    // 直接整体 splice 一次即可，然后只需把 chain 里每个祖先盒的 size 字段 +insert.length）
    const out = Buffer.concat([
        buf.subarray(0, loc.insertOff),
        insert,
        buf.subarray(loc.insertOff)
    ])

    // 祖先盒大小 +insert.length（含采样条目与 moov）
    for (const hdrOff of loc.chain) {
        addBoxSize(out, hdrOff, insert.length)
    }

    // 若 moov 在 mdat 之前，插入使 mdat 后移 → 需调整块偏移
    if (moov.off < mdat.off) {
        adjustChunkOffsets(out, moov.off, moov.off + moov.size + insert.length, insert.length)
    }

    fs.writeFileSync(mp4Path, out)
    return { inserted: insert.length, maxCll, maxFall }
}

module.exports = { injectHdrBoxes, DEFAULT_MASTERING }

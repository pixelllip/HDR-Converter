const fs = require('fs')
const path = require('path')
const gm = require('../../tests/hdr-gainmap.js')
const ROOT = path.join(__dirname, '..', '..')
const buf = fs.readFileSync(path.join(ROOT, 'experiments/gamut_research/cmp_uhdr.jpg'))
const r = gm.extractJpegGainMap(buf)
console.log('result', r ? Object.keys(r) : null)
if (r) {
  console.log('hasGainMap', r.hasGainMap, 'dims', r.gainMapWidth, r.gainMapHeight, 'comp', r.numComponents)
  console.log('meta', JSON.stringify(r.metadata, null, 1))
  if (r.gainMapBase64) fs.writeFileSync(path.join(ROOT, 'experiments/gamut_research/cmp_gainmap.jpg'), Buffer.from(r.gainMapBase64, 'base64'))
  fs.writeFileSync(path.join(ROOT, 'experiments/gamut_research/cmp_meta.json'), JSON.stringify(r.metadata, null, 1))
}

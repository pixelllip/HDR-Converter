const fs=require('fs'), path=require('path'), gm=require('../../tests/hdr-gainmap.js')
const ROOT=path.join(__dirname,'..','..')
for (const g of ['1.0','0.8','1.2']) {
  const buf=fs.readFileSync(path.join(ROOT,`experiments/gamut_research/uhdr_g${g}.jpg`))
  const r=gm.extractJpegGainMap(buf)
  console.log(g,'maxBoost',Math.pow(2,r.metadata.gainMapMaxLog2).toFixed(3),'capMax',Math.pow(2,r.metadata.hdrCapacityMaxLog2).toFixed(3))
  fs.writeFileSync(path.join(ROOT,`experiments/gamut_research/uhdr_gm_${g}.jpg`),Buffer.from(r.gainMapBase64,'base64'))
}

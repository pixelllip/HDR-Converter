import sys
d=open('experiments/gamut_research/edid_active.bin','rb').read()
print('len',len(d),'ext',d[126])
print('mfg', chr(((d[8]>>2)&0x1f)+64)+chr((((d[8]&3)<<3)|(d[9]>>5))+64)+chr((d[9]&0x1f)+64))
# base block: detailed timing descriptors
for i in range(4):
    o=54+i*18
    tag=d[o:o+2]
    if tag==b'\x00\x00':
        kind=d[o+3]
        print(f'  descriptor {i}: tag=0x{kind:02X}')
        if kind==0xFD:
            print('    range limits: minV',d[o+5],'maxV',d[o+6],'minH',d[o+7],'maxH',d[o+8],'maxclk',d[o+9]*10,'mV')
            print('    timing support:',d[o+10:o+18].hex())
    else:
        print(f'  descriptor {i}: DTD pixel clock {int.from_bytes(tag,"little")*10} kHz')
# CTA extension blocks
for e in range(1, d[126]+1):
    o=128*e
    if o+128>len(d): break
    tag=d[o]
    if tag==0x02:
        rev=d[o+1]; dtd=d[o+2]
        print(f'CTA ext {e}: rev={rev} dtd_offset={dtd}')
        p=o+4
        end=o+dtd if dtd>4 else o+127
        while p<end:
            h=d[p]; t=h>>5; ln=h&0x1f
            if ln==0: break
            payload=d[p+1:p+1+ln]
            if t==7 and len(payload)>0:
                ext=payload[0]
                if ext==0x06:
                    eotf=payload[1] if len(payload)>1 else 0
                    smd=payload[2] if len(payload)>2 else 0
                    print(f'  HDR static metadata: eotf=0x{eotf:02X} static_meta=0x{smd:02X} payload={payload.hex()}')
                    if len(payload)>=4: print('    desired max lum =',payload[3],'nits')
                    if len(payload)>=5: print('    desired max frame-avg lum =',payload[4],'nits')
                    if len(payload)>=6: print('    desired min lum =',payload[5]/6.5535,'nits (code',payload[5],')')
                elif ext==0x05:
                    print('  colorimetry:',payload.hex())
                elif ext==0x07:
                    print('  video capability:',payload.hex())
                else:
                    print(f'  extended tag 0x{ext:02X}: {payload.hex()}')
            p+=1+ln
    elif tag==0x70:
        print(f'DisplayID ext {e}')
    else:
        print(f'ext {e}: tag 0x{tag:02X}')

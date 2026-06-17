你还可以使用如下命令，利用ffmpeg将HDR图片转变成可用wallpaper engine播放的壁纸：

```
bash

ffmpeg -loop 1 -i "C:\Users\Administrator\Documents\output.png" ^
-t 3 ^
-c:v libx265 -pix_fmt yuv420p10le -crf 16 ^
-color_primaries bt2020 -color_trc smpte2084 -colorspace bt2020nc ^
-x265-params "colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,1):hdr10=1" ^
-tag:v hvc1 ^
"C:\Users\Administrator\Documents\HDRconvert\output_hdr_3s.mp4"
```
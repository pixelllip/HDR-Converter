#!/usr/bin/env python3
"""
将 PTX 文件转换为 C 头文件, 嵌入 CUDA 内核代码。
用法: python ptx_to_header.py input.ptx output.h
"""

import sys
import os

def main():
    if len(sys.argv) < 3:
        print("Usage: ptx_to_header.py input.ptx output.h")
        sys.exit(1)

    input_path = sys.argv[1]
    output_path = sys.argv[2]

    if not os.path.exists(input_path):
        print(f"Input file not found: {input_path}")
        sys.exit(1)

    with open(input_path, 'r') as f:
        ptx_content = f.read()

    # Escape for C string
    escaped = []
    for c in ptx_content:
        if c == '"':
            escaped.append('\\"')
        elif c == '\n':
            escaped.append('\\n"\n    "')
        elif c == '\\':
            escaped.append('\\\\')
        else:
            escaped.append(c)

    ptx_escaped = ''.join(escaped)

    var_name = os.path.splitext(os.path.basename(input_path))[0]

    with open(output_path, 'w') as f:
        f.write(f'// Auto-generated from {input_path}\n')
        f.write(f'#ifndef HDR_GPU_PTX_EMBED_H\n')
        f.write(f'#define HDR_GPU_PTX_EMBED_H\n\n')
        f.write(f'static const char g_ptx_{var_name}[] =\n')
        f.write(f'    "{ptx_escaped}";\n\n')
        f.write(f'static const size_t g_ptx_{var_name}_size = sizeof(g_ptx_{var_name}) - 1;\n\n')
        f.write(f'#endif\n')

    print(f"PTX embedded header generated: {output_path}")

if __name__ == '__main__':
    main()

cmake_minimum_required(VERSION 3.14)

# hdr_gpu 插件 - 提供 GPU 加速的 HDR 转换
# 支持 DirectCompute (所有 Windows GPU) 和 CUDA (NVIDIA GPU)
#
# 构建流程:
#   1. compile_shaders.bat 编译 HLSL → .cso
#   2. (可选) 如果有 CUDA Toolkit, compile_cuda.bat 编译 .cu → .ptx
#   3. CMake 构建 DLL

project(hdr_gpu LANGUAGES CXX)

# 设置 C++17
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# 源文件
set(HDR_GPU_SOURCES
    src/hdr_gpu.cpp
    src/directcompute_backend.cpp
    src/cuda_backend.cpp
)

set(HDR_GPU_HEADERS
    include/hdr_gpu.h
    src/directcompute_backend.h
    src/cuda_backend.h
)

# DirectX 依赖
# Windows SDK 路径由 Visual Studio 生成器自动提供, 无需手动 find_package
# d3d11.h / d3dcompiler.h 包含在 Windows SDK 中

# 构建设置
add_library(hdr_gpu SHARED ${HDR_GPU_SOURCES} ${HDR_GPU_HEADERS})

target_include_directories(hdr_gpu PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/include
    ${CMAKE_CURRENT_SOURCE_DIR}/src
)

# 定义导出宏
target_compile_definitions(hdr_gpu PRIVATE HDR_GPU_EXPORTS)

# 压制 C4819 警告 (源文件中含中文注释, 当前代码页无法表示)
target_compile_options(hdr_gpu PRIVATE /wd"4819")

# 链接 DirectX
target_link_libraries(hdr_gpu PRIVATE
    d3d11.lib
    d3dcompiler.lib
)

# ============================================
# CUDA 支持 (可选)
# ============================================
# 如果 CUDA Toolkit 可用, 编译 CUDA 内核并嵌入 PTX
# 否则仅构建 DirectCompute 后端

find_program(NVCC nvcc)

if(NVCC)
    message(STATUS "CUDA Toolkit found: ${NVCC}")

    # 编译 CUDA 内核为 PTX
    set(CUDA_KERNEL_SRC ${CMAKE_CURRENT_SOURCE_DIR}/src/cuda_kernels.cu)
    set(CUDA_PTX_OUT ${CMAKE_CURRENT_BINARY_DIR}/cuda_kernels.ptx)
    set(PTX_HEADER ${CMAKE_CURRENT_BINARY_DIR}/cuda_ptx_embed.h)

    add_custom_command(
        OUTPUT ${CUDA_PTX_OUT}
        COMMAND ${NVCC} -ptx ${CUDA_KERNEL_SRC} -o ${CUDA_PTX_OUT}
        DEPENDS ${CUDA_KERNEL_SRC}
        COMMENT "Compiling CUDA kernels to PTX..."
    )

    # 用 CMake 脚本嵌入 PTX (不依赖 xxd/python)
    set(EMBED_SCRIPT ${CMAKE_CURRENT_SOURCE_DIR}/tools/embed_ptx.cmake)
    add_custom_command(
        OUTPUT ${PTX_HEADER}
        COMMAND ${CMAKE_COMMAND} -D PTX_FILE=${CUDA_PTX_OUT}
                               -D HEADER_FILE=${PTX_HEADER}
                               -P ${EMBED_SCRIPT}
        DEPENDS ${CUDA_PTX_OUT}
        COMMENT "Embedding PTX as C header..."
    )

    # 将生成的 PTX 头文件加入构建
    target_include_directories(hdr_gpu PRIVATE ${CMAKE_CURRENT_BINARY_DIR})
    target_compile_definitions(hdr_gpu PRIVATE HAS_CUDA_PTX=1)

    # PTX 头文件是 hdr_gpu 的依赖项
    add_dependencies(hdr_gpu generate_cuda_ptx)
    add_custom_target(generate_cuda_ptx DEPENDS ${PTX_HEADER})
else()
    message(STATUS "CUDA Toolkit not found. Building with DirectCompute only.")
endif()

# ============================================
# 嵌入编译后的着色器 CSO (如果存在)
# ============================================
# 尝试嵌入预编译的 .cso 文件
# 如果不存在, DirectCompute 后端会自动在运行时内联编译 HLSL

set(CSO_DIR ${CMAKE_CURRENT_SOURCE_DIR}/shaders)
set(CSO_PASS1 ${CSO_DIR}/pass1_srgb_to_linear.cso)
set(CSO_PASS2 ${CSO_DIR}/pass2_apply_hdr.cso)

if(EXISTS ${CSO_PASS1} AND EXISTS ${CSO_PASS2})
    # 使用 file2c 或类似工具嵌入 CSO
    # 如果没有预编译, 运行时内联编译 HLSL 源码
    message(STATUS "Pre-compiled shaders found, will use them")
    # 目前使用运行时编译, 以简化构建流程
endif()

# 安装目标 (输出到 Flutter 插件目录)
install(TARGETS hdr_gpu
    RUNTIME DESTINATION ${CMAKE_INSTALL_PREFIX}
    LIBRARY DESTINATION ${CMAKE_INSTALL_PREFIX}
)

# CMake script to embed PTX file as C header
# Usage: cmake -D PTX_FILE=input.ptx -D HEADER_FILE=output.h -P embed_ptx.cmake

if(NOT EXISTS "${PTX_FILE}")
    message(FATAL_ERROR "PTX file not found: ${PTX_FILE}")
endif()

file(READ "${PTX_FILE}" PTX_CONTENT)

# Escape special characters for C string
string(REPLACE "\\" "\\\\" PTX_CONTENT "${PTX_CONTENT}")
string(REPLACE "\"" "\\\"" PTX_CONTENT "${PTX_CONTENT}")
string(REPLACE "\n" "\\n\"\n\"" PTX_CONTENT "${PTX_CONTENT}")

get_filename_component(VAR_NAME "${PTX_FILE}" NAME_WE)

set(HEADER_CONTENT "// Auto-generated from ${PTX_FILE}
#ifndef HDR_GPU_PTX_EMBED_H
#define HDR_GPU_PTX_EMBED_H

static const char g_ptx_${VAR_NAME}[] =
    \"${PTX_CONTENT}\";

static const size_t g_ptx_${VAR_NAME}_size = sizeof(g_ptx_${VAR_NAME}) - 1;

#endif
")

file(WRITE "${HEADER_FILE}" "${HEADER_CONTENT}")
message(STATUS "PTX embedded header generated: ${HEADER_FILE}")

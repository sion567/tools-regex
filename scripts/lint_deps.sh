#!/usr/bin/env bash
# 依赖纪律检查（设计方案 6.4，tools-gis lint:deps 思路复刻）
# 规则：
#   1. regex-ir 零 [dependencies]
#   2. 各 crate 只允许白名单依赖
#   3. 只有 app 可以依赖 eframe/egui
# 用法：bash scripts/lint_deps.sh   （CI / 提交前跑）
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
fail=0

# 提取某 crate 的 [dependencies] 字段名列表
deps_of() {
    awk '/^\[dependencies\]/{f=1;next} /^\[/{f=0} f && NF && $1 !~ /^#/ {print $1}' "$1"
}

contains() { # contains "item" "list..."（空格分隔）
    local item="$1"; shift
    for x in "$@"; do [[ "$x" == "$item" ]] && return 0; done
    return 1
}

check() { # check <crate> <允许的依赖...>
    local crate="$1"; shift
    local toml="$root/crates/$crate/Cargo.toml"
    local bad=""
    while IFS= read -r dep; do
        [[ -z "$dep" ]] && continue
        if ! contains "$dep" "$@"; then
            bad="$bad $dep"
        fi
    done < <(deps_of "$toml")
    if [[ -n "$bad" ]]; then
        echo "FAIL  $crate 出现白名单外依赖:$bad（允许：$*）"
        fail=1
    fi
}

check regex-ir
check regex-parser regex-ir
check regex-plain regex-ir
check regex-diagram regex-ir
check regex-export regex-ir regex-diagram
check regex-exec regex-ir regex fancy-regex memmap2 rayon
# wasm-api：VSCode 插件/浏览器的粘合层（wasm-bindgen 仅 target 段，不在 [dependencies]）
check wasm-api regex-ir regex-parser regex-plain regex-diagram regex-exec regex-export serde serde_json
# app 是壳：允许全部内部 crate + eframe，但也别引其他 UI 框架
check app regex-ir regex-parser regex-plain regex-diagram regex-exec regex-export eframe egui

# UI 禁令：除 app 外任何 crate 提到 egui/eframe 即违规（双保险，白名单已覆盖）
for c in regex-ir regex-parser regex-plain regex-diagram regex-exec regex-export wasm-api; do
    if grep -qE '^\s*(egui|eframe)' "$root/crates/$c/Cargo.toml"; then
        echo "FAIL  $c 不得依赖 egui/eframe"
        fail=1
    fi
done

if [[ "$fail" -eq 0 ]]; then
    echo "PASS  依赖纪律检查通过（regex-ir 零依赖；UI 只在 app；各 crate 白名单内）"
fi
exit "$fail"

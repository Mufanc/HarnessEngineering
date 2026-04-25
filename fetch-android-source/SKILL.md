---
name: fetch-android-source
description: 根据 cs.android.com 链接获取指定行号范围的 Android 源码片段。当用户提供 cs.android.com 链接并需要查看、引用对应源码，或在写博客时需要插入 Android 源码片段时使用。
---

# 获取 Android 源码片段

## 触发条件

- 用户提供了 cs.android.com 链接，需要查看对应源码
- 用户在写博客时需要引用 Android 源码
- 用户要求获取某个 Android 源文件的指定行

## 限制

- 仅支持带行号（`;l=`）的链接，不支持搜索 URL
- 需要网络访问 android.googlesource.com

## 工作流程

### 1. 提取 URL

从用户消息中提取 cs.android.com 链接。

### 2. 运行脚本

脚本位于本 SKILL.md 同级的 `scripts/fetch.py`，接收一个 cs.android.com 链接作为参数：

```bash
python3 scripts/fetch.py "<URL>"
```

示例：

```bash
python3 scripts/fetch.py "https://cs.android.com/android/platform/superproject/main/+/main:frameworks/base/core/java/android/app/LoadedApk.java;l=1042-1046;drc=50f34b45baed2ec3a256f1c65df4865d72452768"
```

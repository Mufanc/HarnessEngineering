# 辩论工作流系统 (DebateLLM)

## 概述

本项目是一个基于辩论的智能体工作流系统，通过多个子智能体的协作，对用户提交的问题进行多角度、深入的辩论分析。

## 子智能体架构

```
用户提交问题
      │
      ▼
┌─────────────┐
│  Analyzer   │  分析问题，生成辩论框架
│ (分析器)    │  输出: analysis.md
└──────┬──────┘
       │
       ▼
┌──────────────────────────────────────┐
│         辩论循环 (最多20轮)          │
│                                      │
│  ┌─────────┐        ┌─────────┐      │
│  │   Pro   │◄──────►│   Con   │      │
│  │ (正方)  │  辩论  │ (反方)  │      │
│  └────┬────┘        └────┬────┘      │
│       │                  │           │
│       ▼                  ▼           │
│  round_N_pro.md    round_N_con.md    │
│  memory_pro.md     memory_con.md     │
│       │                  │           │
│       └────────┬─────────┘           │
│                ▼                     │
│         ┌──────────┐                 │
│         │  Judge   │  评判 + 更新    │
│         │ (评判官) │  state.md       │
│         └────┬─────┘                 │
│              │                       │
│     continue_debate?                 │
│       │          │                   │
│      Yes        No                   │
│       │          │                   │
│  下一轮(优势     │                   │
│  方先发言)       │                   │
└──────────────────────────────────────┘
                   │
                   ▼
       ┌────────────────────────┐
       │  总结陈词阶段          │
       │  Pro → closing_pro.md  │
       │  Con → closing_con.md  │
       └───────────┬────────────┘
                   │
                   ▼
            ┌──────────────┐
            │  Summarizer  │  整合关键材料，生成报告
            │  (总结者)    │  输出: final_report.md
            └──────────────┘
```

## 子智能体说明

| 子智能体 | 文件 | 职责 |
|---------|------|------|
| debate-analyzer | `.qoder/agents/debate-analyzer.md` | 接收用户问题，分析并生成辩论议题和双方提示词 |
| debate-pro | `.qoder/agents/debate-pro.md` | 正方辩手，进行论证和反驳，维护 memory_pro.md |
| debate-con | `.qoder/agents/debate-con.md` | 反方辩手，进行论证和反驳，维护 memory_con.md |
| debate-judge | `.qoder/agents/debate-judge.md` | 每轮评判，判断共识或继续辩论，更新 state.md，确定下一轮发言顺序 |
| debate-summarizer | `.qoder/agents/debate-summarizer.md` | 基于总结陈词和最后一轮发言生成最终报告 |

## 辩论工作流编排指南

当用户提交问题需要辩论分析时，按以下步骤编排工作流：

### 第一步：初始化工作目录

```
为每次辩论创建独立的工作目录：
debates/
  {timestamp}_{topic_slug}/
    analysis.md            # 分析器输出
    memory_pro.md          # 正方辩论记忆
    memory_con.md          # 反方辩论记忆
    rounds/
      round_1_pro.md       # 第1轮正方
      round_1_con.md       # 第1轮反方
      round_1_judge.md     # 第1轮评判
      round_2_pro.md       # 第2轮正方
      ...
    closing_pro.md         # 正方总结陈词
    closing_con.md         # 反方总结陈词
    final_report.md        # 最终报告
    state.md               # 工作流状态(用于中断恢复)
```

### 第二步：问题分析

使用 `debate-analyzer` 子智能体，传入以下提示词：

```
请分析以下辩论问题，并将结果写入 {work_dir}/analysis.md：

问题：{用户的问题}

辩论工作目录：{work_dir}
```

### 第三步：辩论循环

```python
# 伪代码
max_rounds = 20
next_first_speaker = "pro"  # 默认正方先发言

for round_num in range(1, max_rounds + 1):
    
    # 根据上一轮评判决定发言顺序
    if next_first_speaker == "pro":
        first_side, second_side = "pro", "con"
    else:
        first_side, second_side = "con", "pro"
    
    # 先发言方
    invoke(f"debate-{first_side}", prompt=f"""
        请进行第 {round_num} 轮辩论的{first_side}方发言。
        辩论工作目录：{work_dir}
        轮次：{round_num}
        你是本轮先发言方（上一轮优势方）。
        {"对方上一轮发言：" + read(f"rounds/round_{round_num-1}_{second_side}.md") if round_num > 1 else "这是第一轮，请阐述你的初始立场。"}
    """)
    
    # 后发言方
    invoke(f"debate-{second_side}", prompt=f"""
        请进行第 {round_num} 轮辩论的{second_side}方发言。
        辩论工作目录：{work_dir}
        轮次：{round_num}
        对方本轮发言：{read(f"rounds/round_{round_num}_{first_side}.md")}
    """)
    
    # 评判（Judge 会同时更新 state.md）
    invoke("debate-judge", prompt=f"""
        请对第 {round_num} 轮辩论进行评判。
        辩论工作目录：{work_dir}
        轮次：{round_num}
    """)
    
    # 读取评判结果，获取下一轮发言顺序
    judgment = read(f"rounds/round_{round_num}_judge.md")
    
    if not judgment.continue_debate:
        break
    
    # 从 state.md 读取下一轮先发言方
    state = read(f"state.md")
    next_first_speaker = state.next_first_speaker  # "pro" 或 "con"
```

### 第四步：总结陈词

辩论环节结束后，分别让正方和反方做总结陈词：

```python
# 正方总结陈词
invoke("debate-pro", prompt=f"""
    辩论环节已结束，请做总结陈词。
    辩论工作目录：{work_dir}
    请将总结陈词写入 {work_dir}/closing_pro.md
""")

# 反方总结陈词
invoke("debate-con", prompt=f"""
    辩论环节已结束，请做总结陈词。
    辩论工作目录：{work_dir}
    请将总结陈词写入 {work_dir}/closing_con.md
""")
```

### 第五步：生成总结

```
invoke("debate-summarizer", prompt=f"""
    辩论已结束，请生成最终总结报告。
    辩论工作目录：{work_dir}
    最后一轮轮次：{last_round_num}
""")
```

### 第六步：返回结果

读取 `final_report.md` 的内容并呈现给用户。

## 中断恢复机制

`state.md` 文件记录工作流的当前状态，由 Judge 在每轮评判后更新：

```markdown
# 辩论状态

- **辩论ID**：20260411_topic_slug
- **状态**：in_progress
- **当前轮次**：3
- **当前阶段**：judging_done
- **已完成轮数**：3
- **下一轮先发言方**：con
- **开始时间**：2026-04-11T12:00:00Z
- **最后更新**：2026-04-11T12:05:30Z
```

恢复时读取 `state.md`，从记录的轮次和阶段处继续执行。

## 使用方式

在 Qoder 中直接描述你的辩论问题即可，例如：

```
请对以下问题进行辩论分析：远程工作是否应该成为企业的默认工作模式？
```

系统会自动编排上述子智能体完成完整的辩论工作流。

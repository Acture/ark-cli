<div align="center">

# Ark CLI

**明日方舟玩家的工具箱**

[![Release](https://img.shields.io/github/v/release/Acture/ark-cli?filter=ark-cli-v*&label=release)](https://github.com/Acture/ark-cli/releases?q=ark-cli-v&expanded=true)
[![Platform](https://img.shields.io/badge/platform-Windows%20x64-0078D6)](#安装)
[![License](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue)](LICENSE)

[快速上手](#快速上手) · [能做什么](#能做什么) · [常见问题](#常见问题) · [English](README.en.md)

</div>

找到一份作业，却缺了其中一位干员？不知道自己的练度够不够，或者想看看技能晚开一点还能不能过？

Ark CLI 把这些问题做成一组小工具：把作业跑一遍、试着降低练度、看看操作能晚多少，也尝试为已有作业寻找干员替换方案。

目前通过命令行使用。原版战斗功能需要 Windows 和已安装的官方 PC 客户端。

## 能做什么？

- **先跑一遍作业。** 导入 MAA 作业，看看能不能过、在哪里漏怪，也可以打开窗口观看战斗。
- **试试更低的练度。** 调整等级、专精、模组和信赖，看看哪些配置仍然能过，不必只照着满配练。
- **看看操作能晚多少。** 给操作加一点延迟，或试几个技能释放时机，了解这份作业对操作时机有多挑剔。
- **缺干员时，找找替换方案。** 基于已有作业尝试搜索替换，并检查换人后的结果。这项功能仍在完善，只支持部分机制和配置。

## 快速上手

### 安装

原版战斗功能需要 **Windows x64**，以及通过鹰角启动器安装、保持更新的官方《明日方舟》PC 客户端。

1. 从 [Releases](https://github.com/Acture/ark-cli/releases?q=ark-cli-v&expanded=true) 下载 `ark-cli-<版本>-windows-x86_64.zip`，解压到任意文件夹。每个压缩包旁都有 `.sha256` 校验文件。
2. 在这个文件夹里打开终端（资源管理器中右键空白处 →“在终端中打开”）。下文命令中的 `ark-cli`，在这里要写成 `.\ark-cli`；把这个文件夹加入 PATH 后，就可以直接用 `ark-cli`。

Releases 里的“战斗运行包”会在第一次准备环境时自动下载，不需要手动下载，也不能单独使用。Linux 和 macOS 版本只包含离线工具，原版战斗功能仅支持 Windows。

<details>
<summary>从源码安装</summary>

准备 Rust 1.89 或更新版本、MSVC C++ 构建工具、Windows SDK，以及 PATH 中的 FlatBuffers 编译器 `flatc` 25.12.19。编译 `ark-cli` 本身不需要安装 Unity 编辑器。

```powershell
git clone https://github.com/Acture/ark-cli.git
cd ark-cli
cargo install --locked --path src\ark_cli
ark-cli --version
```

</details>

### 跑你的第一份作业

**1. 准备环境。**

```powershell
ark-cli battle setup
```

工具会寻找客户端、下载战斗运行包并准备游戏数据，全部保存在当前目录的 `assets\battle-runtime\` 下（可用 `--state` 修改）。找不到客户端时，可以手动指定安装目录：

```powershell
ark-cli battle setup --client "G:\Hypergryph Launcher\games\Arknights"
```

客户端更新后，重新执行 `setup` 即可；仍然有效的准备步骤会复用。

**2. 准备作业。** 使用已有的 MAA 作业文件，或从 [PRTS Plus](https://prts.plus) 获取一份，保存为 `job.json`。作业文件是记录部署、技能、撤退等步骤的 JSON，不是攻略视频。

**3. 开始战斗。**

```powershell
ark-cli battle run job.json
```

想边跑边看，可以打开战斗窗口：

```powershell
ark-cli battle run job.json --presentation watch --speed 2
```

> **先注意练度：** 默认按最高练度生成队伍（满级、专精三、三级模组、满信赖、满潜能），不会读取你的账号阵容。要测试自己的配置，请明确设置练度或提供队伍文件。

## 再试试这些

**用自己的练度跑。** 下例让每位干员在最高精英化阶段取 60 级（不超过该阶段上限）、专精一、不装模组：

```powershell
ark-cli battle run job.json --level 60 --mastery 1 --module 0
```

不同干员的配置不一样？生成队伍文件，编辑后再运行：

```powershell
ark-cli battle squad job.json squad.json
ark-cli battle run job.json --squad squad.json
```

**看看练度还能降多少。** 尝试寻找仍能无漏怪通关的较低练度：

```powershell
ark-cli battle min-training job.json --search bisect --per-operator
```

**看看操作有多紧。** 测试统一操作延迟，或尝试几个技能延后释放的时机：

```powershell
ark-cli battle latency job.json --max-delay 120
ark-cli battle tune-skills job.json --waits 0,15,30,60,90
```

游戏速度为 1 时，30 个战斗 tick 对应 1 秒游戏时间。练度和时机搜索只在各自的范围与假设下探索，不保证找到所有配置中的最优解。

**找找干员替换方案。** 这项实验功能需要另外准备版本匹配的数据与战斗资源，只支持部分机制。使用方法见 [离线工具说明](src/ark_cli/README.md)。

每个命令都可以加 `--help` 查看全部选项，例如 `ark-cli battle run --help`。

## 结果怎么看？

命令会在终端输出 JSON 报告，并把详细记录保存到 `assets\battle-runtime\runs\<时间>\`。

先看 `report.json` 中的 **`battle_outcome.status`**：`original_win` 是获胜，`original_loss` 是失败。报告还会记录剩余生命、漏怪和每一步操作的执行时间。

**退出码 0 或 `passed: true` 不等于通关**，它们表示运行通过了执行检查。一次正常结束的失败战斗，也可能通过这些检查。

获胜的作业会生成 `concrete-job.json`，记录本次使用的练度与操作，MAA 可以直接执行；自然结束且通过终态检查的战斗还会生成 `battle-proof.json`。

## 常见问题

**需要装 Unity 吗？**
不需要安装 Unity 编辑器。战斗运行包里带有运行所需的 Unity Player，`setup` 会自动下载。只有自行构建运行包时，才需要 Unity 2021.3.39f1、Windows IL2CPP 模块、有效的编辑器许可证、Unity CLI 和 MSVC。离线工具不需要 Unity。

**它会替我操作游戏、消耗理智吗？**
不会。战斗在独立的运行环境中执行，不操作官方客户端，不需要登录游戏账号，也不会替你完成账号里的关卡。

**这里赢了，实际游戏就一定能过吗？**
不能保证。工具使用原版战斗代码，但运行环境和操作方式与官方客户端并不完全相同。延迟测试也不等于完整模拟人工操作或 MAA 的截图识别。

**所有作业、干员和关卡都支持吗？**
不是。原版战斗与离线分析各有支持范围；无法处理的输入会报错或报告未完成，不能把它解释为“能过”或“过不了”。离线搜索生成的候选也必须经过回放检查，搜索本身不是通关证明。

## 更多文档

- [战斗运行指南](src/battle_runtime/README.md)：队伍配置、战斗程序格式、运行模式与结果文件。
- [CLI 与离线工具](src/ark_cli/README.md)：数据准备、替换搜索、回放与复核。
- [源码构建与发布](src/ark_cli/RELEASE.md)
- [参与开发](CONTRIBUTING.md)

使用中遇到问题，可在 [Issues](https://github.com/Acture/ark-cli/issues) 反馈。请附上命令、错误信息与相关版本，不要上传账号信息或客户端文件。

## 许可与声明

本项目自有代码采用 **GNU AGPL v3 或后续版本（AGPL-3.0-or-later）**，详见 [LICENSE](LICENSE)。按 AGPL 第 7 节，[NOTICE](NOTICE) 另外允许把本项目与 Unity Player 运行时及官方客户端的代码和数据组合使用并分发。Unity 运行时和其他第三方组件适用各自的许可，见 [第三方声明](src/battle_runtime/licenses/THIRD_PARTY.md)。

本项目是独立工具，不是鹰角网络的官方产品。本仓库及发布包不包含《明日方舟》客户端、游戏资源或游戏数据快照。请仅使用合法拥有的客户端文件，不要提交或分发本地提取的游戏数据。

《明日方舟》名称及相关内容的权利归其权利人所有；本项目的许可不授予这些内容的使用或分发权利。

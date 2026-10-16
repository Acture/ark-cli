<div align="center">

# Ark CLI

**Your Arknights toolbox**

[![Release](https://img.shields.io/github/v/release/Acture/ark-cli?filter=ark-cli-v*&label=release)](https://github.com/Acture/ark-cli/releases?q=ark-cli-v&expanded=true)
[![Platform](https://img.shields.io/badge/platform-Windows%20x64-0078D6)](#install)
[![License](https://img.shields.io/badge/license-AGPL--3.0--or--later-blue)](LICENSE)

[Quick start](#quick-start) · [Features](#what-can-it-do) · [FAQ](#faq) · [简体中文](README.md)

</div>

Found a guide, but missing an operator? Not sure your team is built enough, or whether a skill can go off a little later and still clear?

Ark CLI brings together tools for those questions: run a guide, try lower operator investment, test how much delay it can tolerate, and explore replacements for an existing team.

It currently runs from the command line. Original-engine battles require Windows and an installed official PC client.

## What can it do?

- **Run a guide first.** Import an MAA job, check whether it clears and where enemies leak, or open a window to watch the battle.
- **Try lower investment.** Adjust levels, skill mastery, modules and trust to see which setups still clear.
- **Check the timing window.** Add input delay or try later skill activations to see how demanding the guide's timing is.
- **Look for operator replacements.** Search for alternatives in an existing guide and check the revised result. This experimental feature supports only a limited set of mechanics and configurations.

## Quick start

### Install

Original-engine battles require **Windows x64** and an up-to-date official Arknights PC client installed through the Hypergryph Launcher.

1. Download `ark-cli-<version>-windows-x86_64.zip` from [Releases](https://github.com/Acture/ark-cli/releases?q=ark-cli-v&expanded=true) and unpack it into any folder. Each archive has a `.sha256` checksum beside it.
2. Open a terminal in that folder (right-click an empty area in File Explorer → "Open in Terminal"). There, type the `ark-cli` in the commands below as `.\ark-cli`; add the folder to `PATH` to use plain `ark-cli`.

The "battle runtime" on the Releases page is downloaded automatically the first time you set up. You don't need to download it yourself, and it cannot be used on its own. Linux and macOS builds contain only the offline tools; original-engine battles are Windows-only.

<details>
<summary>Build from source</summary>

Install Rust 1.89 or newer, MSVC C++ build tools, a Windows SDK, and FlatBuffers `flatc` 25.12.19 on `PATH`. Building `ark-cli` itself does not require the Unity Editor.

```powershell
git clone https://github.com/Acture/ark-cli.git
cd ark-cli
cargo install --locked --path src\ark_cli
ark-cli --version
```

</details>

### Run your first guide

**1. Prepare the environment.**

```powershell
ark-cli battle setup
```

This finds the client, downloads the battle runtime and prepares the game data, all under `assets\battle-runtime\` in the current directory (change it with `--state`). If the client is not found, specify its installation directory:

```powershell
ark-cli battle setup --client "G:\Hypergryph Launcher\games\Arknights"
```

Rerun `setup` after a client update. Completed steps are reused while they remain valid.

**2. Get a job file.** Use an existing MAA job or get one from [PRTS Plus](https://prts.plus), then save it as `job.json`. This is a JSON file describing deployments, skills and retreats, not a guide video.

**3. Start the battle.**

```powershell
ark-cli battle run job.json
```

To watch it run, open the battle window:

```powershell
ark-cli battle run job.json --presentation watch --speed 2
```

> **Check the squad first:** by default the team is generated at maximum investment (max level, mastery 3, module level 3, max trust and potential). The tool does not read your account's roster. Set the investment explicitly or provide a squad file to test your own setup.

## Try a few more things

**Run with your own investment.** This example uses level 60 at each operator's highest elite phase (capped by that phase's maximum), mastery 1 and no module:

```powershell
ark-cli battle run job.json --level 60 --mastery 1 --module 0
```

Need different settings for each operator? Generate a squad file, edit it and run again:

```powershell
ark-cli battle squad job.json squad.json
ark-cli battle run job.json --squad squad.json
```

**See how much investment you can reduce.** Search for a lower setup that still clears without leaks:

```powershell
ark-cli battle min-training job.json --search bisect --per-operator
```

**Check how tight the timing is.** Test a uniform input delay or several later skill timings:

```powershell
ark-cli battle latency job.json --max-delay 120
ark-cli battle tune-skills job.json --waits 0,15,30,60,90
```

At game speed 1, 30 battle ticks equal one second of game time. Investment and timing searches work within their stated ranges and assumptions; they do not guarantee the best result across every possible configuration.

**Explore operator replacements.** This experimental workflow needs matching game-data and battle-resource snapshots and supports only some mechanics. See the [offline tools guide](src/ark_cli/README.md).

Add `--help` to any command for all of its options, for example `ark-cli battle run --help`.

## Read the result

Commands print a JSON report and save detailed records under `assets\battle-runtime\runs\<time>\`.

Start with **`battle_outcome.status`** in `report.json`: `original_win` means a win; `original_loss` means a loss. The report also records remaining life points, leaks and the tick each operation was issued at.

**Exit code 0 or `passed: true` does not mean the battle was won.** It means execution checks passed. A battle that naturally ends in a loss can still pass those checks.

A winning job produces `concrete-job.json`, recording the investment and operations used, which MAA can execute directly. Battles that end naturally and pass terminal-state checks also produce `battle-proof.json`.

## FAQ

**Do I need Unity installed?**
No, you don't need the Unity Editor. The battle runtime contains the Unity Player it needs, and `setup` downloads it. Building the runtime yourself requires Unity 2021.3.39f1, the Windows IL2CPP module, a valid Editor license, Unity CLI and MSVC. The offline tools don't need Unity.

**Will it play on my account or spend sanity?**
No. Battles run in a separate runtime environment. It doesn't operate the official client, log into your game account or complete stages on it.

**If it wins here, will it definitely win in the game?**
No. The tool uses the original battle code, but its environment and input delivery differ from the official client. Delay tests are not a complete simulation of manual play or MAA's screen recognition.

**Does it support every guide, operator and stage?**
No. Original-engine battles and offline analysis have separate support limits. Unsupported inputs produce errors or incomplete reports, not a claim that a guide wins or loses. Offline search candidates must also pass replay checks; searching alone does not prove a clear.

## More documentation

- [Battle runtime guide](src/battle_runtime/README.md): squads, the battle-program format, presentation modes and result files.
- [CLI and offline tools](src/ark_cli/README.md): data preparation, replacement search, replay and validation.
- [Source builds and releases](src/ark_cli/RELEASE.md)
- [Contributing](CONTRIBUTING.md)

Report problems in [Issues](https://github.com/Acture/ark-cli/issues), including the command, the error and the relevant versions. Don't upload account information or client files.

## License and notices

First-party code is licensed under the **GNU AGPL v3 or later (AGPL-3.0-or-later)**; see [LICENSE](LICENSE). Under AGPL section 7, [NOTICE](NOTICE) additionally permits combining this project with the Unity Player runtime and with code and data from an official client, and conveying the result. The Unity runtime and other third-party components remain under their own terms; see the [third-party notices](src/battle_runtime/licenses/THIRD_PARTY.md).

This is an independent project, not an official Hypergryph product. The repository and releases contain no Arknights client, game assets or game-data snapshots. Use only client files you legally own, and don't commit or redistribute extracted game data.

Arknights and its related content belong to their respective rights holders. This project's license grants no rights to use or distribute that content.

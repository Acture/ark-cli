# Third-party components

`ark-cli battle setup` places these components in its owned state directory. A release built with
the `bundled-tools` feature embeds them; other builds download the pinned upstream wheel and
verify every hash. They restore stripped engine field descriptions; they never run game logic.

| Component | Version | Files | Source | License |
| --- | --- | --- | --- | --- |
| UnityPy type tree data | 1.25.4 | `lzma.tpk` | `unitypy-1.25.4-cp312-cp312-win_amd64.whl` (PyPI) | MIT |

## UnityPy

MIT License

Copyright (c) 2019-2026 K0lb3

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Owned Unity Player

The prebuilt owned Player is built with Unity 2021.3.39f1 from this repository's Unity template.
Its Unity runtime files are distributed under the Unity terms that apply to built Players.
It contains no Arknights client file; `setup` copies the user's own installed client at run time.

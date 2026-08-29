# 预注册签署记录 — sylvode.flow.coldstart.v1

**状态：`APPROVED_FOR_FORMAL_RUN`**

- reviewer：OpenAI Codex（独立 reviewer，**未编写本测量 runner**）
- 签署时点：**2026-08-29T13:41:56Z**（America/New_York 09:41:56-04:00）
- 时序事实：签署发生在所批准的正式 run **开始之前**；审查方未执行任何正式或 smoke run
- 审查报告：`/opt/worker/report/codex-adr0015-r7-presign-2026-08-29.md`

## 签署绑定的三个 hash（任一字节变更，本签署即失效）

| 对象 | SHA-256 |
|---|---|
| `decisions/ADR-0015-cold-start-measurement-split.md`（R7，301 行） | `7b6725f4f09fc5804f3e7cfede99c4233cc9825f9b3dd6107c284970907aadb5` |
| `probe/coldstart-probe-v1.json` | `6fb9ed16e62e2a5714f5268f3c8cceb58bd0cfef740d38e6833145320184c386` |
| `probe/calibration-256k.bin`（262,144 B） | `e23d9c02e637b253337bf7dee349235783b6a63a059934fe526c5ba7f1329904` |

**本签署记录刻意不写进 ADR 正文**——写进去会改变 ADR 的字节内容，使签署绑定的 hash 失配、签署自我作废。

## 审查轨迹

| 轮次 | 时点 | 所审 hash（前 16 位） | 决定 |
|---|---|---|---|
| R4 | 2026-08-29T12:46:16Z | `f9d680579e650ece` | `NOT_SIGNED_HOLD` — 四项具体判据未进入被签文本 |
| R5 | 2026-08-29T13:24:21Z | `316f613bd2c41f7a` | `NOT_SIGNED_HOLD` — 吞吐公式确定性假阴性且候选间不对称；CRDT 核对口径自相矛盾 |
| R7 | 2026-08-29T13:41:56Z | `7b6725f4f09fc580` | **`APPROVED_FOR_FORMAL_RUN`** |

## reviewer 附带的约束（正式 run 期间必须遵守）

「独立字节数与 `encodedDataLength` 精确相等」这条判据**存在操作脆弱性**：
裸 socket 带 `Connection: close`，浏览器的 request headers 与连接复用语义不同，
通用 HTTP server 可能据此生成不同长度的响应头。在 R7 冻结的环境下可接受
（响应体/Content-Type/Content-Length/Cache-Control/identity 编码固定，日期字段定宽），
且冒烟中两个 origin 反复等于 262,317 B。

**关键约束**：若该精确相等在正式 run 中失稳，结果是 **fail-closed 判 `invalid`，不是假绿**。
reviewer 给出了更稳健的替代判据（改用浏览器实得 identity body 的固定字节数作分子），
但明确规定：**切换该替代会改变已签协议，必须形成新 hash 并重新预签，
且不得在正式结果出现之后切换。**

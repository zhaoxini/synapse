# 固定本机 pairing code 设计

Date: 2026-07-05

## 目标

简化 Synapse 登录和配对：普通用户不需要记 relay 地址，也不需要每次输入 email/password。首次缺少本地配置时交互式登录；登录成功后本机保存账号配置和 6 位 pairing code。之后 server 启动直接复用本地配置和固定 code，并向 relay 注册当前 `code -> device_id` 映射。

## 接受标准

1. `synapse-server login` 不带参数也可运行，交互式询问 email/password；`--relay/--email/--password` 保留给自动化。
2. 默认 relay 地址来自打包默认值；用户正常路径不需要输入 relay。
3. `synapse-server` 启动时，如果 `~/.synapse/config.json` 存在，不再询问 email/password。
4. 如果 config 不存在，启动时进入交互式登录；登录成功写入 config。
5. 首次登录成功后生成 6 位数字 code，保存到 `~/.synapse/pairing-code`。
6. 后续启动复用同一个本机 code，不自动轮换。
7. 每次 server 启动和 relay 注册失败后的重试，都用本机固定 code 向 relay 注册 `code -> device_id`。
8. 手机用 6 位 code 连接时，relay 能找到对应在线机器；机器离线时兑换失败。
9. 重新登录会生成新 code，覆盖旧 code，并注册新映射。
10. 现有 LAN/local web `http://127.0.0.1:8000/?code=xxxxxx` 仍可用。

## 设计

### CLI 登录

把 `login` 子命令参数从必填改为可选：

- `--relay` 可选，缺省用 `account::default_relay_url()`。
- `--email` 可选，缺省交互式输入。
- `--password` 可选，缺省交互式输入且不回显。
- `--device-name` 保留可选，缺省用当前机器名。

`register` 可先保持原状，避免扩大改动。

### 本地配置

继续使用现有 `~/.synapse/config.json` 保存登录后的 relay/device token。启动时已有 config 就直接进入 run flow。

新增一个小函数：

- `load_or_create_pairing_code(reset: bool) -> Result<String>`

行为：

- `reset=false`：有 `~/.synapse/pairing-code` 就复用；没有就生成 6 位数字并保存。
- `reset=true`：生成新 6 位数字并覆盖。

登录成功路径使用 `reset=true`。普通启动使用 `reset=false`。

### Relay 注册 code

现有 `create_pairing_code` 由 relay 生成短期 code。改为本机提供 code，让 relay 只登记映射：

- 请求体包含 `code`。
- relay 保存/刷新 `code -> device_id`。
- 返回同一个 code。

server 启动时：

1. 读取本机固定 code。
2. 调用 relay 注册 code。
3. 打印 code 和 web URL。
4. 后台定时重试注册同一个 code，处理 relay 重启或网络抖动。

不再自动换 code。

### Relay 行为

relay 的 pairing code store 以 code 为 key，value 指向 device/account。注册同一个 code 时覆盖旧映射，适配重新启动和重新登录。

离线语义不靠 code 过期实现：兑换 code 后仍要连设备；设备不在线则失败。

### 错误处理

- 缺少本地 config：交互式登录。
- relay 注册 code 失败：server 继续运行本地 web/API，日志警告，并后台重试。
- code 文件损坏或不是 6 位数字：生成新 code 并覆盖。
- login 失败：不覆盖现有 config/code。

## 测试和验证

- Rust tests：
  - `login` 参数可选解析。
  - pairing code 首次生成后复用。
  - `reset=true` 重新生成。
  - 损坏 code 文件会修复。
  - relay pairing registration 接受 caller-provided code。
- Runtime verification：
  - 删除 `~/.synapse/config.json` 后启动，看到交互式登录提示。
  - 登录成功后 `~/.synapse/pairing-code` 存在。
  - 重启 server 后 code 不变。
  - `synapse-server pairing-code` 打印同一个 code。
  - web URL 使用同一个 code。

## 非目标

- 不做长期云账号管理 UI。
- 不改手机端完整登录流程，除非当前 code exchange 结构必须随 relay 响应小改。
- 不把 code 存 DB；本轮按用户确认，code 源头在本机。

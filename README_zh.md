# draftnote

[English](README.md) | **中文**

跨平台的单人 Markdown 写作客户端。笔记平铺存放、用 tag 分类、并在多设备间同步，形态参照 Simplenote 和 Joplin。运行于 macOS、Windows 和 iOS。

![draftnote 桌面版](screenshot/draftnote-desktop.jpg)

## 桌面版功能

写作：

- 用 Markdown 写作；笔记涉及代码时可切换为 Shell 或 JSON，配套语法高亮
- 实时预览，渲染 Markdown 与 Mermaid 图
- 笔记内查找，并在匹配之间跳转
- 字号可调，明暗主题，均会记住

整理：

- 所有笔记平铺成一个列表，可按 tag 或全文搜索过滤
- 可置顶、复制、移入垃圾箱；支持多选批量操作
- 垃圾箱保留 90 天后自动清除
- tag 以标签块形式在编辑器上方添加
- 可导入单个文件或整个文件夹；任一视图可导出为 Markdown 或纯文本

历史：

- 每次保存都留存一个版本；打开历史即可看到改了什么、何时改的
- 随时恢复到较早版本，且不会丢掉更新的版本

同步：

- 笔记通过你自己的 GitHub 仓库或自建服务器在多设备间保持同步
- 编辑即时保存，稍后自动上传；其他设备各自拉取
- 同一篇在两处被修改时，两个版本会一起保留，供你手动合并

## 移动端（iOS）

相对桌面端的界面差异：

- 三个全屏视图（tag 与目录、笔记列表、编辑器）一次显示一个，每屏左上角都有返回按钮
- 编辑器保留返回按钮，把 tag 编辑、字号、预览、主题控件收进一个菜单
- 预览时可双指缩放
- 导入 / 导出仅在桌面端

## 使用

1. 启动应用。上次打开的那篇自动打开，焦点在编辑器。
2. **新建笔记**：点列表栏顶部的铅笔图标。正文第一个非空行成为标题（Shell 和 JSON 笔记用导入时的文件名做标题）。
3. **切换类型**：右键笔记可在 Markdown、Shell、JSON 之间切换。编辑器语言和预览高亮跟着变，导出扩展名也随之调整。
4. **加 tag**：在编辑器上方的 tag 输入框敲入并回车（点已有 pill 可删除）。tag 会作为分组出现在左栏。
5. **笔记内查找**：点编辑工具栏的放大镜图标，或按 `Ctrl/Cmd+F`。匹配处高亮；右侧滚动条上每个匹配一个 tick，点 tick 跳过去。
6. **预览**：点眼睛图标切换预览面板。Markdown 里 Mermaid 图实时渲染； Shell 和 JSON 以带语法高亮的代码块呈现。
7. **Pin / Duplicate / History / Trash**：在列表里右键。History 打开版本面板；Delete Permanently 只在垃圾箱视图出现。Ctrl/Cmd+点击或 Shift+点击可多选，右键可批量丢入垃圾箱、恢复、彻底删除。
8. **导入 / 导出**：导航栏底部有 Import Notes 按钮（从目录或从单个文件导入），识别 `.md`、`.txt`、`.sh`、`.json` 四种扩展名；`.sh` / `.json` 会保留类型并按同一扩展名导出。右键导航项（All Notes、 Trash、Untagged 或任一 tag）可把该视图导出为 Markdown 或纯文本。
9. **建私有 GitHub 仓库**（用自建后台或已有仓库可跳过）：头像下拉菜单 → Repositories（在 Profile 下方）→ New。Owner 选自己的账号。Choose visibility 下 Private 是唯一选项。勾选 Add a README。其他保持默认。

   ![建私有 GitHub 仓库](screenshot/draftnote_repo.jpg)
10. **同步**：点齿轮图标打开 Sync Settings。

    填写：

    - **Repo URL** — 仓库的浏览器地址（`https://github.com/user/repo` 或你自建服务器上的对应地址）。base URL、owner、repo 从中派生。
    - **Username** 与 **Token** — 你的凭据。
    - **Server URL** *（可选）* — 覆盖从 Repo URL 派生的 base URL。当后台部署的地址与 repo 主机不同时用（比如 `http://10.0.1.244:8015`）。
    - **API mode** — 开：走 GitHub REST API，只用 token 认证（`github.com` 改写为 `api.github.com`，自建服务器保留自身域名）。关：自建服务器，讲同一套 API，用 Basic username:token 认证。

    **校验与保存**：填了 token 时，Save 在 **Validate** 通过前保持禁用；Validate 会做一次完整同步。

    **使用中**：编辑即时存本地，停止输入几秒后推送到远端；指向同一 repo 的其他设备各自按节奏拉取。

    **存储**：配置在 `~/.draftnote/config.json`（token 加密存储，非明文）；笔记在 `~/.draftnote/notes/` 下。

## 自建后台

`backend/` 目录里是一个 Go 小服务，提供客户端讲的同一套 Contents API 子集，数据是磁盘上一个普通 git 仓库。客户端指向它、在 Settings 里把 API mode 关掉即可，认证走 Basic username:token。

环境变量（全部可选，一个都不设也能跑，采用下方默认值）：

- `DRAFTNOTE_DATA` *（可选，默认 `/data`）* — 工作区目录，克隆放在 `<data>/draftnote`
- `DRAFTNOTE_REMOTE_URL` *（可选）* — 远程 git 地址，`<data>/draftnote` 不存在时用它 clone；已存在则原样使用，不做删除后重建
- `DRAFTNOTE_ADDR` *（可选，默认 `:8080`）* — 监听地址，形如 `:8080` 或 `0.0.0.0:9000`
- `DRAFTNOTE_PORT` *（可选）* — 监听端口，形如 `8015`；当 `DRAFTNOTE_ADDR` 已设时忽略
- `DRAFTNOTE_USERNAME` *（可选）* — 服务端准入的用户名。客户端要在 Sync Settings 里填同一个值，发出的 Basic-auth 头才能通过
- `DRAFTNOTE_TOKEN` *（可选）* — 与上面用户名配对的 token；开着 API mode 的客户端也可以只带 `Authorization: token <token>` 认过。留空时服务器完全跳过认证，接受任意请求（内网私用可以，公网暴露千万别这么开）

挂载目录结构：

```
<data>/                     挂载点(DRAFTNOTE_DATA,默认 /data)
└── draftnote/              服务操作的 git 仓库
    ├── .git/               完整 git 历史(版本记录在这里)
    └── notes/
        ├── <id>.json       每篇笔记一个文件
        └── …
```

`<data>/draftnote` 是一个真正的 git 工作区，HEAD 始终保持在 `draftnote` 分支；每次 PUT/DELETE 就是该分支上的一次 commit。如果你不用 `DRAFTNOTE_REMOTE_URL` 而是自己把工作区预先放在 `<data>/draftnote`，它必须是一个含 `notes/` 的普通 git 仓库。

本地构建运行：

```
cd draftnote/backend
go build -o draftnote-backend .
DRAFTNOTE_DATA=/tmp/dn-data \
DRAFTNOTE_USERNAME=alice \
DRAFTNOTE_TOKEN=s3cret \
./draftnote-backend
```

`-addr` 是监听地址，`-port` 是监听端口，默认 `:8080`。也可以用环境变量 `DRAFTNOTE_ADDR`、`DRAFTNOTE_PORT`。

一条命令同时出二进制和 Docker 镜像（产物为 `draftnote-backend` 与本地镜像 `draftnote-backend:latest`）：

```
cd draftnote/backend
bash build.sh
docker run --rm -p 8080:8080 \
  -v $PWD/data:/data \
  -e DRAFTNOTE_USERNAME=alice \
  -e DRAFTNOTE_TOKEN=s3cret \
  -e DRAFTNOTE_REMOTE_URL=https://github.com/user/repo.git \
  draftnote-backend:latest
```

`$PWD/data` 是挂载的工作区目录。首次启动时服务会 clone `DRAFTNOTE_REMOTE_URL` 到 `$PWD/data/draftnote`；后续启动直接复用，不会重新下载。如果你已经把 git 工作区放在 `$PWD/data/draftnote` 里，可以不给 `DRAFTNOTE_REMOTE_URL`。

如果客户端的 Repo URL 指向的远端与服务器 clone 的远端不一致，Sync Settings 里的 Validate 会提示，并给出两个选择："Keep server"（记住这个决定，下次不再提示）或 "Reclone from Repo URL"（销毁本地 clone 后按客户端 Repo URL 重新 clone）。

服务器也接受 `Authorization: token <token>` 头，方便 API mode 开着但指向自建 URL 的客户端连过来；这种情况下用户名不校验。

## 构建

工具链：

- Rust（stable）
- Node 20+
- Tauri CLI：`cargo install tauri-cli --version '^2'`

安装前端依赖：

```
cd draftnote/rust-tauri/frontend
npm install
```

桌面开发：

```
cd draftnote/rust-tauri
cargo tauri dev --features desktop
```

桌面构建：

```
cd draftnote/rust-tauri
cargo tauri build --features desktop
```

iOS：

```
cd draftnote/rust-tauri
cargo tauri ios init
cargo tauri ios build
```

测试（客户端核心与后端）：

```
cd draftnote/rust-tauri/src-tauri && cargo test
cd draftnote/backend && go test ./...
```

正文加密：在 Sync Settings 中输入 6 位 PIN，可加密存放在 Git 远端的笔记正文；留空则关闭加密。本地笔记仍以明文保存。切换加密状态或更换 PIN 时，会全量重写远端笔记。PIN 连续验证失败三次后锁定 5 分钟，重启应用后仍然有效。加密仅覆盖正文，标题、tag、时间戳和文件类型仍可在仓库中查看。6 位 PIN 用于避免正文被直接目视阅读，不用于抵御离线猜测。

CI 配置：[.github/workflows/draftnote_build.yml](../.github/workflows/draftnote_build.yml)。

## 许可证

GNU 通用公共许可证 v3.0 或更高版本。见 [LICENSE](LICENSE)。

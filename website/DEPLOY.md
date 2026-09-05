# 网站部署信息

本目录（`website/`）是 Kivio 官网静态站，已部署在下面这台服务器上。

## 服务器

- 商家：GreenCloudVPS（海外，无需 ICP 备案）
- SSH：默认 `root@185.200.65.236`，使用 SSH Key 登录
- 系统：Debian 12

## 部署方式

复用服务器上已有的 **Caddy 网关**（Docker，容器名 `caddy-gateway`），网站作为一个静态站点块挂进去，Caddy 自动签发 / 续期 Let's Encrypt 证书。

- 网站文件目录（服务器）：`/opt/domain-gateway/sites/kivio-desktop/`
- Caddy 配置：`/opt/domain-gateway/Caddyfile`（本站的块见 `kivio-desktop.xyz` / `www.kivio-desktop.xyz`）
- compose：`/opt/domain-gateway/docker-compose.yml`（挂载 `./sites:/srv:ro`）
- 同机还跑着 `sub2api` + `api.zmfooogreencloud.xyz`，**别动**。

## 域名

- `kivio-desktop.xyz`（阿里云注册 + 解析）
- 解析：`@` 和 `www` 两条 A 记录 → `185.200.65.236`
- HTTPS：Let's Encrypt，自动续期
- `www` 会 301 跳到 apex

## 更新网站

插件格式与工作流 Hook 两页以仓库 `docs/agents/kivio-plugin-format.md`、`docs/agents/plugin-hooks.md` 为内容来源。修改规范或 `docs/schemas/kivio-plugin.v1.schema.json` 后，在仓库安装开发依赖并运行 `node scripts/sync-plugin-docs.mjs`，同步官网文章及可下载 Schema。不要直接编辑这两篇生成的 HTML。

改完本目录内容后，跑：

```bash
./website/deploy.sh
```

即时生效，Caddy 无需重启。

首次使用前，将部署公钥加入服务器的 `authorized_keys`。如需覆盖默认目标，可设置：

```bash
KIVIO_DEPLOY_HOST=user@example.com \
KIVIO_DEPLOY_DEST=/path/to/site \
./website/deploy.sh
```

> 注意：deploy.sh 用 tar 覆盖上传，**不会删除**服务器上已删掉的文件。若要严格同步删除，改用 rsync --delete。

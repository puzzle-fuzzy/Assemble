# 邮箱与 GitHub 账户配置

账户规则以[决策 0005](../decisions/0005-public-email-accounts-and-github.md)为准：公开注册、验证邮箱、网页改密与找回、可选 GitHub 登录。配置文件不能提交进公开仓库。

## SMTP

在部署的受保护 `.env` 中填写以下变量；在编辑器或自己的终端中输入授权码，不通过命令参数传入，不打印整份环境。

```dotenv
SMTP_HOST=smtp.163.com
SMTP_PORT=465
SMTP_SECURE=true
SMTP_USER=your-account@163.com
SMTP_PASS=
SMTP_FROM="Impact <your-account@163.com>"
```

163 使用 SMTP 授权码，不是邮箱网页登录密码。`SMTP_FROM` 的邮箱应与发信账户一致。465 的 `true` 是 SSL/TLS 直连；选择 `false` 时使用 STARTTLS，仍要求加密，不会降级为明文。六项要么全部填写，要么全部不填；缺项或非法格式会让服务启动失败。未配置时仍可打开注册页，但请求发信会明确提示暂不可用，不会跳过验证。

也可以用 `SMTP_PASS_FILE` 代替 `SMTP_PASS`，用 `GITHUB_CLIENT_SECRET_FILE` 代替 `GITHUB_CLIENT_SECRET`，值是受保护密钥文件的绝对路径。每项值与文件引用二选一，不能同时配置。文件由 Rust 读取，Shell 与启动参数不用接触密钥明文；容器中使用时须把文件只读挂载到容器路径，并让容器的 10001 账户可读。文件仅包含密钥本身，可有末尾换行，不超过 4 KiB。

Linux：`.env` 属于部署用户，权限设为 `600`；备份同样保护。Windows 本地测试只放在已忽略的 `data/local/` 下，ACL 限制当前用户与 SYSTEM。不要把 SMTP 授权码粘进日志、截图或测试断言。`check-config` 只报告是否配好，不显示密码。

### 容器使用密钥文件

部署目录同时放入 `compose.yaml` 和 `compose.secrets.yaml`，在该目录下创建 `secrets/`。通过受保护的文件传输把授权码与 OAuth Secret 分别保存为 `secrets/smtp-password`、`secrets/github-client-secret`，不要在命令参数中输入内容。目录与文件属主均为 `10001:10001`，目录权限 `0700`、文件权限 `0400`。叠加配置以只读方式挂载整个目录；目录不存在时直接报错，不自动创建空目录。

Linux 正式服务器的 `.env` 配置：

```dotenv
COMPOSE_FILE=compose.yaml:compose.secrets.yaml
SMTP_PASS_FILE=/run/assemble-secrets/smtp-password
GITHUB_CLIENT_SECRET_FILE=/run/assemble-secrets/github-client-secret
```

删除互斥的 `SMTP_PASS` 和 `GITHUB_CLIENT_SECRET`，其余连接参数照常填写。之后在该目录运行 `docker compose config --quiet`、`docker compose up -d`、`docker compose exec -T assemble assemble-server check-config`。`COMPOSE_FILE` 会让后续重建继续使用挂载；复制配置备份和回滚时也要保留对应文件。上述冒号分隔适用于 Linux；Windows 本机检查时明确使用 `-f compose.yaml -f compose.secrets.yaml`。

## 创建 GitHub OAuth App

1. 在 GitHub 打开 [Developer settings → OAuth Apps](https://github.com/settings/developers)，选择 **New OAuth App**。
2. Application name 填 `Assemble`，Homepage URL 填 `https://as.yxswy.com`。
3. Authorization callback URL 填 **`https://as.yxswy.com/web/github/callback`**，不要填桌面回环地址，也不要填写带查询串的登录地址。
4. 创建后复制 Client ID，并生成 Client Secret。只在服务器的受保护 `.env` 配置：

   ```dotenv
   GITHUB_CLIENT_ID=
   GITHUB_CLIENT_SECRET=
   ```

5. 本地和预览环境各创建独立 OAuth App，把 Homepage 和 callback 改成该环境的真实来源；本机示例是 `http://127.0.0.1:8787/web/github/callback`。不能把正式 Secret 放进桌面构建或 `VITE_*`。
6. 按部署手册更新镜像和重建服务，检查注册页、邮箱验证和 GitHub 按钮。GitHub 页面只应要求读取账户资料和邮箱，不应索取仓库权限。

实现使用 GitHub 的 [OAuth Web Flow、state 与 PKCE](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps)，读取 [已验证邮箱](https://docs.github.com/en/rest/users/emails)。没有配置完整两项时 GitHub 登录入口隐藏；邮箱注册不受影响。

## 升级与历史兼容

2026-11-25 已按用户要求清空正式账号及关联数据，当前新用户直接从 `/web/register` 或 GitHub 入口注册，无需补绑旧账号。下面保留兼容实现的技术说明，不代表要恢复已删除的正式账号。

升级前按[部署手册](deploy.md)备份。新迁移原地增加邮箱、头像、外部身份、邮件凭据和限流表，不重建账户或迁移云盘。`ASSEMBLE_INVITE_CODE` 不再读取，应从旧 `.env` 删除。

旧用户名账户打开 `/web/legacy`，本人输入原用户名、原密码与邮箱，收信验证后使用邮箱登录。不要重建同名账户代替绑定，否则原文件仍属于旧账户。运维的 `create-account` 仍可创建待绑定的旧式账户；公开用户应从网页注册，不再通过它绕过验证。

身份绑定和更换邮箱需要同一浏览器确认。邮件在另一浏览器打开时，可复制邮件中的验证码，回到申请邮件的原浏览器打开 `/web/verify` 粘贴。不要把验证码交给别人，也不要替不认识的 GitHub 身份确认注册。

GitHub 与现有账户同邮箱时不会自动合并：先用邮箱密码登录已有账户，在 `/web/account` 输入当前密码后点“绑定 GitHub”。GitHub 名称和邮箱将来改变，仍按同一个 GitHub 数字 id 登录原账户。更换 Assemble 主邮箱从账户页操作，先后验证原、新邮箱。

## 代理与验收

`deploy/nginx.conf.example` 为 GitHub callback 单独关闭访问日志，避免单次 OAuth code 落盘。修改已有共享 nginx 必须先取得授权、备份配置，执行 `nginx -t` 通过后再 reload。不要为了此项顺手修改其他站点。

验收分别记录：

- `bun run verify` 与两平台 CI；测试用替身邮件箱和 GitHub 提供商，覆盖并发、过期、重放、相同邮箱不合并、旧账户绑定、双邮箱确认、密码找回。
- 本地真实启动和系统浏览器：邮箱登录表单、注册入口、验证片段填表、确认提交、默认头像；桌面仍经浏览器授权。
- SMTP 真发送：只能向自己控制的收件箱发验证邮件，不能以 SMTP 登录成功替代实际收信。不要在正式环境建临时账户。
- GitHub 真授权：用户配置 OAuth App 后亲自登录；核对可选择主邮箱、头像、既有账户绑定及再次登录的归属。

安装包、代码推送、服务端部署与真实第三方验收是不同步骤，报告时明确哪些已经完成。

首次注册或绑定邮箱若超过桌面端原有的 5 分钟等待，网页验证仍可完成；随后重新点桌面的“使用浏览器登录”，已完成的邮箱验证不用重做。

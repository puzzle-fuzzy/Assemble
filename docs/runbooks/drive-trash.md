# 云盘回收站

N3 源码实现本人文件及文件夹的回收、整批恢复和明确永久删除。本轮只在隔离环境验证，没有部署正式服务或发布、安装桌面更新；iOS、APNs 和推送未改动。依赖 N2 的目录模型，发布时先升级服务端，再发布桌面更新。

## 使用与权限

云盘内提供“文件 / 回收站”两个页签，沿用共享表格、弹窗和顶部面包屑。桌面默认每页 10，管理端 20；搜索作用于当前层级，输入末尾图标清空。空表只有文字。恢复与永久删除各自确认，移入和永久删除默认聚焦取消。中文和英语系统文案同步，文件名及目录名原样显示，日期继续使用本机时区。

删除一个活动文件夹时，它当时仍活动的后代形成一个批次；此前单独回收的子项保持自己的批次，不被父级永久清理带走。每批最多 5000 条实际记录，目录最多 16 层，账户文件夹上限仍包含回收中的目录。批次及已处理回执合计每账户最多 10000 条，达到上限只拒绝新的移入，不阻止已有批次恢复或永久删除。当前不自动过期、不提供一键清空。

恢复保留原标识、字节和内部层级。原父目录可用时恢复到其当前位置；原父失效或层级超限时恢复到根目录。文件夹重名生成“（恢复）”“（恢复 2）”等合法后缀，按 UTF-16 长度截取，不覆盖、合并或改动另一份目录。确认前显示实际位置和必要的改名；目标变化要求重新核对。成功提示可“查看位置”。

管理端从云盘所属账户入口查看回收站，只有元数据及层级，没有恢复、永久删除、预览或下载。管理员不能代替用户清空。本人使用设备身份；管理端逐次检查已验证邮箱、超级管理员和同源 HttpOnly 会话，设备令牌不能使用管理接口。响应为 no-store；不返回内容摘要、令牌、密码、服务器路径或带凭据的地址。

## HTTP 契约

权威类型在 `crates/assemble-protocol/src/drive_trash.rs`。时间使用 Timestamp（RFC3339），版本为正整数，资源标识不能当作本机路径。

| 入口                                                                            | 结果或输入                                                                                 |
| ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| `POST /v1/drive/trash/preview`                                                  | 输入 `{kind,id,version}`；返回 target、name、fileCount、folderCount、size、revision        |
| `POST /v1/drive/trash`                                                          | 输入 `{target,revision,requestId}`；返回 Batch                                             |
| `GET /v1/drive/trash?q=&page=1&pageSize=20`                                     | `{items:Batch[],total,page,pageSize}`                                                      |
| `GET /v1/drive/trash/{id}?parentId=root&q=&page=1&pageSize=20`                  | `{batch,current,ancestors,items,total,page,pageSize}`；只读该批次层级                      |
| `POST /v1/drive/trash/{id}/restore-preview`                                     | 返回 batchId、id、kind、name、parentId、reason、renamed、version、revision、parentLocation |
| `POST /v1/drive/trash/{id}/restore`                                             | 输入 `{version,revision}`；返回实际恢复的标识、父级、名称及原因                            |
| `DELETE /v1/drive/trash/{id}?version=`                                          | 返回 `{id,logicalBytesReleased,cleanupPending}`，不把逻辑释放等同于磁盘已释放              |
| `GET /v1/admin/drive/trash?accountId=&q=&page=1&pageSize=20`                    | 按所属账户只读批次                                                                         |
| `GET /v1/admin/drive/trash/{id}?accountId=&parentId=root&q=&page=1&pageSize=20` | 按所属账户只读层级；没有管理员写接口                                                       |

Batch 字段：id、rootId、kind（file/folder）、name、originalLocation（显示用名称数组）、deletedAt、version、fileCount、folderCount、size。层级条目只包含 id、parentId、kind、name、size（文件夹为 null）、version、createdAt。reason 为 original、parentUnavailable 或 depthLimit。分页每页 1～100，搜索最多 200 字符；批次按删除时间和标识降序，内部层级文件夹优先，再按创建时间和标识降序。

revision 只是并发校验值，不是授权凭据。移入的修订覆盖整个活动子树，不能只检查根版本；恢复修订绑定实际父级、名称和目标同级目录。写入用 BEGIN IMMEDIATE 并重新核验身份。相同 requestId/输入只返回同一未处理批次；已恢复、已永久删除的移入请求不能重放。恢复与永久删除保留最小幂等回执，重复请求不会再次改变内容或扣引用。

旧 `/v1/drive/files/{id}` DELETE 和 N2 文件删除在升级后只移入，成功仍为 204；N2 文件夹删除仍只接受空目录。无版本旧入口对曾恢复的文件返回 DRIVE_TRASH_UPGRADE_REQUIRED，避免迟到旧重试再次删除；带版本旧请求由递增版本拒绝。新桌面及助手空目录删除先探测能力，旧服务不支持就拒绝，不回退永久删除。

常见错误：VERSION_CONFLICT、DRIVE_TRASH_CHANGED 要重新核对；DRIVE_TRASH_FINISHED 表示回执已处理；DRIVE_TRASH_TOO_LARGE/DRIVE_TRASH_LIMIT 表示实际记录上限；跨账户或非活动条目为 NOT_FOUND。所有活动读取、内容、预览、Range 和条件读取在服务端过滤回收状态，不能靠 UI 隐藏。提交前已经送达的字节和本机保存副本无法收回，原有设备发送的独立引用继续有效。

## 存储与维护

新增迁移固定 `0016_drive_trash.sql`，已有迁移校验不变。`drive_trash` 保存批次及回执，原记录的 trash_id 关联批次，根记录从旧父级脱离但保留恢复信息；活动目录唯一索引只约束未回收目录。active_drive_files/active_drive_folders 供活动读取使用，原配额仍统计活动加回收的文件。

永久删除先在同一事务内删除该批次记录、精确扣引用并登记 blob_cleanup，再由每分钟维护轮执行实体删除。磁盘失败保留任务并增加 attempts；首次失败与次数变更同事务写业务审计，后续重试不重复刷屏。启动后的维护继续任务；同摘要重新上传先持有内容租约，直到登记引用后才释放，清理会重新核对并取消误删。其他账户、活动文件或待接收发送仍引用的内容不会被删除。

AdminStorage 增加 trashBytes（逻辑用量）与 reclaimableBytes（去重后的预计可释放物理内容）。后者只统计全部剩余引用都在回收站、没有活动文件和待接收发送的摘要；不包含尚未实际清理的磁盘增量。旧服务缺少字段时界面显示未知。现有全站预算、磁盘可用和数据库统计维持原口径。

日志动作包括 drive.trashed/restored/purged、drive.folder_trashed/folder_restored/folder_purged，失败清理为 drive.cleanup_failed 或 drive.folder_cleanup_failed；保存操作者、设备和目标标识，文件历史名称由既有安全快照保留。不得手改业务库或用清空数据解决上限。

## 桌面与助手

桌面通过八个具名命令完成 list、detail、cancel、preview、move、restore-preview、restore、purge；旧读取取消机制与会话 epoch 同步，退出清空回收快照，迟到旧结果不能写回下一账户。快照样本新增 driveTrash；旧 DriveFile/DriveListing 线格式不变，iOS 样本不变。

MCP 新增 assemble_drive_trash_list 与 assemble_drive_trash_restore，总计 27 个具名工具。list 无 id 读分页名单，带 id 获取真实恢复方案；恢复要求用户明确意图以及最新 version/revision。没有永久删除工具，也不开放原文、上传、下载或任意路径。

## 验证与发布

实际结果记录于本机 `data/local/n3/acceptance.md`。发布前必须完成 bun run verify、真实 Docker 构建启动，并分别记录 Windows WebView、管理浏览器、Mac 和两台真实设备结果。尚未完成的平台不能用自动化或其他平台代替。

先备份数据库（含回收状态、回执和任务）及 blobs，核对迁移 1～15 校验，再应用 16；检查活动/回收数量、逻辑字节、引用、内容校验及恢复流程。实体删除后仅靠数据库旧备份无法找回文件，恢复必须使用同一时点内容备份。迁移后回退应恢复匹配数据库与 blobs，不让旧程序直接操作新增回收记录。正式升级按部署手册，桌面签名发布仍只在发布 Mac 上手动执行。本轮没有自动发布。

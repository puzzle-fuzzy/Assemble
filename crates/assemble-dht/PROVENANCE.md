# DHT 实现来源

- 参考 `E:\dht-observer`，提交 `31809a05a13d41597374ec2d8dcf575b7e07f72e`，包括元数据验证、采样预算、任务状态与管理功能。
- 参考其 `engines/dht-spider`，以及 `E:\dht-spider` 的提交 `430bc8746e2892d6f84c6dbaee3788d1c8626b90`。
- 原项目：[adysec/dht-spider](https://github.com/adysec/dht-spider)，作者 AdySec，Copyright (c) 2025，MIT。原许可证保留为 `LICENSE.spider`。

本目录将原有 bencode、KRPC 与 BEP-9/10 帧和分片处理改写为工作区 Rust 库，并加入 Observer 的 BEP-51 采样、原始摘要验证、公网地址检查与有界解析规则。进程编排与持久化归 Assemble 服务端，未保留原独立网页、密钥认证、SQLite 连接或日志轮转。未复制旧数据库与部署凭据。

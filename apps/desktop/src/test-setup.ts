// 测试环境的全局准备：注册 DOM 实现，并打开 React 的 act 环境。
import { GlobalRegistrator } from "@happy-dom/global-registrator";

GlobalRegistrator.register({ width: 1024, height: 768 });

// 测试不经过 Vite，用固定构建身份验证展示；真实元数据由工具脚本的测试核对。
Object.assign(globalThis, {
  __ASSEMBLE_BUILD__: { channel: "local", version: "0.1.0", commit: "1234567" },
});

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

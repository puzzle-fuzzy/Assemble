// 测试环境的全局准备：注册 DOM 实现，并打开 React 的 act 环境。
import { GlobalRegistrator } from "@happy-dom/global-registrator";

GlobalRegistrator.register({ width: 1024, height: 768 });

(
  globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

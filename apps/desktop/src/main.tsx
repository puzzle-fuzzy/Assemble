import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "@/App";
import { SendPanel } from "@/SendPanel";
import "@/styles/app.css";

// 同一份页面代码，按地址里的参数决定渲染哪个窗口：主窗口，或者发送面板。
const which = new URLSearchParams(window.location.search).get("window");

const root = document.getElementById("root");
if (!root) throw new Error("缺少 #root 容器");

createRoot(root).render(
  <StrictMode>{which === "send" ? <SendPanel /> : <App />}</StrictMode>,
);

import "@assemble/display";

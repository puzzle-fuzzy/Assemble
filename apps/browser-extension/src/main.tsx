import { createRoot } from "react-dom/client";
import { Popup } from "./Popup";
import "./style.css";

const root = document.getElementById("root");
if (root)
  createRoot(root).render(
    <Popup
      send={(request) => chrome.runtime.sendMessage(request)}
      requestCapture={() =>
        chrome.permissions.request({ origins: ["http://*/*", "https://*/*"] })
      }
    />,
  );

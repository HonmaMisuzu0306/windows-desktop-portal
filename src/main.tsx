import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";

// 桌面挂件不需要右键菜单和网页级拖放
window.addEventListener("contextmenu", (e) => e.preventDefault());
window.addEventListener("dragover", (e) => e.preventDefault());
window.addEventListener("drop", (e) => e.preventDefault());

createRoot(document.getElementById("root")!).render(<App />);

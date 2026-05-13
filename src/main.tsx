import React from "react";
import ReactDOM from "react-dom/client";
import { FluentProvider, webDarkTheme } from "@fluentui/react-components";
import App from "./App";
import "./App.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <FluentProvider theme={webDarkTheme} style={{ backgroundColor: "transparent" }}>
      <App />
    </FluentProvider>
  </React.StrictMode>
);

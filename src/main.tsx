import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";
import { FluentProvider, webLightTheme, webDarkTheme } from "@fluentui/react-components";

// Determine if we should use dark theme based on system preference
const isDarkTheme = window.matchMedia("(prefers-color-scheme: dark)").matches;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <FluentProvider theme={isDarkTheme ? webDarkTheme : webLightTheme}>
      <App />
    </FluentProvider>
  </React.StrictMode>,
);

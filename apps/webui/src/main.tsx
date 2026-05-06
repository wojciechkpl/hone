/* @refresh reload */
import "uno.css";
import "./styles/base.css";
import "./styles/themes/crimson.css";
import "./styles/themes/chalk.css";
import "./styles/themes/neon.css";
import "./styles/themes/minimal.css";

import { render } from "solid-js/web";
import { App } from "./App";

const root = document.getElementById("root");
if (!root) {
  throw new Error("#root not found in index.html");
}
render(() => <App />, root);

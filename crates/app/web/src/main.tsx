import { setupIonicReact } from "@ionic/react";
import { createRoot } from "react-dom/client";

import App from "./App";

import "@ionic/react/css/core.css";
import "@ionic/react/css/normalize.css";
import "@ionic/react/css/structure.css";
import "@ionic/react/css/typography.css";
import "@ionic/react/css/padding.css";
import "@ionic/react/css/float-elements.css";
import "@ionic/react/css/text-alignment.css";
import "@ionic/react/css/text-transformation.css";
import "@ionic/react/css/flex-utils.css";
import "@ionic/react/css/display.css";

import "./theme/variables.css";
import "./theme/github-dark.min.css";
import "./theme/synapse.css";

setupIonicReact({ mode: "ios" });

document.body.classList.add("mode-workspaces");

createRoot(document.getElementById("root")!).render(<App />);

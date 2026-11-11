// Entry point of the browser preview: index.html?mock loads this, which hands the page the
// mock API and the scene player. Not built into the app.

import { call } from "./mock-api.js";
import { play } from "./scenes.js";
import { picture } from "./stand-ins.js";

window.gkPreview = { call, play, picture };

// The stage/rail/drawer arithmetic. Pure. Imports nothing.
export const RAIL_W = 320;
export const RAIL_W_MID = 280; // windows up to 1100 px
export const RAIL_STRIP_W = 56;
export const DRAWER_MIN = 360;
export const DRAWER_DEFAULT = 520;
export const DRAWER_MAX = 1400;

// The least width the pane grid (or the board) may get. One pane: 560 px. Two or four panes sit
// side by side, so each keeps about 380 px.
export function stageFloor(grid) {
  return grid > 1 ? 760 : 560;
}

export function clampDrawer(w) {
  const n = Number(w);
  if (!Number.isFinite(n)) return DRAWER_DEFAULT;
  return Math.min(DRAWER_MAX, Math.max(DRAWER_MIN, Math.round(n)));
}

// vw: window width; grid: panes shown (1 for the board); drawerOpen; drawerW: the person's width;
// railManual: the person collapsed the rail.
// -> { railW, railCollapsed, railAuto, drawerW, overlay }
//   railAuto: the rail is a strip only because the window is too small for rail + stage + drawer.
//   overlay: even with the strip there is no room for the drawer beside the stage, so it floats over it.
export function planLayout({ vw, grid = 1, drawerOpen = false, drawerW = DRAWER_DEFAULT, railManual = false }) {
  const full = vw <= 1100 ? RAIL_W_MID : RAIL_W;
  const floor = stageFloor(grid);
  const want = drawerOpen ? clampDrawer(drawerW) : 0;
  const auto = !railManual && full + floor + want > vw;
  const collapsed = railManual || auto;
  const railW = collapsed ? RAIL_STRIP_W : full;
  if (!drawerOpen) return { railW, railCollapsed: collapsed, railAuto: auto, drawerW: 0, overlay: false };
  const room = vw - railW - floor;
  if (room >= DRAWER_MIN) return { railW, railCollapsed: collapsed, railAuto: auto, drawerW: Math.min(want, room), overlay: false };
  return { railW, railCollapsed: collapsed, railAuto: auto, drawerW: Math.min(want, Math.max(vw - railW, DRAWER_MIN)), overlay: true };
}

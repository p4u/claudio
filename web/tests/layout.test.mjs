import test from 'node:test';
import assert from 'node:assert/strict';
import { planLayout, stageFloor, clampDrawer, DRAWER_MIN, DRAWER_DEFAULT, DRAWER_MAX, RAIL_STRIP_W } from '../app/layout.js';

test('the stage floor is 560 for one pane and wider for a split', () => {
  assert.equal(stageFloor(1), 560);
  assert.ok(stageFloor(2) >= 560);
  assert.equal(stageFloor(4), stageFloor(2));
});

test('the drawer width is clamped to 360..1400 and falls back to 520', () => {
  assert.equal(clampDrawer(100), DRAWER_MIN);
  assert.equal(clampDrawer(5000), DRAWER_MAX);
  assert.equal(clampDrawer('x'), DRAWER_DEFAULT);
  assert.equal(clampDrawer(433.4), 433);
});

test('a big window keeps the full rail beside the drawer', () => {
  const p = planLayout({ vw: 1920, grid: 2, drawerOpen: true, drawerW: 520 });
  assert.deepEqual(p, { railW: 320, railCollapsed: false, railAuto: false, drawerW: 520, overlay: false });
});

test('the rail becomes a 56 px strip when rail + floor + drawer do not fit', () => {
  const p = planLayout({ vw: 1440, grid: 2, drawerOpen: true, drawerW: 520 });
  assert.equal(p.railCollapsed, true);
  assert.equal(p.railAuto, true);
  assert.equal(p.railW, RAIL_STRIP_W);
  assert.equal(p.drawerW, 520);
  assert.equal(p.overlay, false);
  // always at least the floor for the stage
  assert.ok(1440 - p.railW - p.drawerW >= stageFloor(2));
});

test('one pane at 1440 keeps the rail with a default drawer', () => {
  const p = planLayout({ vw: 1440, grid: 1, drawerOpen: true, drawerW: 520 });
  assert.equal(p.railCollapsed, false);
  assert.ok(1440 - p.railW - p.drawerW >= 560);
});

test('the drawer shrinks toward its minimum before it floats', () => {
  const p = planLayout({ vw: 1100, grid: 1, drawerOpen: true, drawerW: 700 });
  assert.equal(p.railCollapsed, true);
  assert.equal(p.overlay, false);
  assert.equal(p.drawerW, 1100 - RAIL_STRIP_W - 560);
});

test('a window too small for strip + floor + minimum drawer floats the drawer', () => {
  const p = planLayout({ vw: 900, grid: 1, drawerOpen: true, drawerW: 520 });
  assert.equal(p.overlay, true);
  assert.equal(p.railCollapsed, true);
  assert.ok(p.drawerW >= DRAWER_MIN && p.drawerW <= 900 - RAIL_STRIP_W);
});

test('a manual collapse holds with no drawer; without a drawer a roomy window keeps the rail', () => {
  assert.equal(planLayout({ vw: 1600, railManual: true }).railCollapsed, true);
  assert.equal(planLayout({ vw: 1600, railManual: true }).railAuto, false);
  assert.equal(planLayout({ vw: 1600 }).railCollapsed, false);
  assert.equal(planLayout({ vw: 1600 }).drawerW, 0);
});

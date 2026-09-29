//! Camera 2D che segue il giocatore con uno smorzamento morbido.
//!
//! Con "Segui" attivo (tasto F o bottone nell'ispettore) la camera segue invece
//! l'NPC selezionato: il giocatore resta dov'è e non può interagire finché
//! non si torna a lui (di nuovo F, o chiudendo l'ispettore). Se l'NPC seguito
//! muore, l'inquadratura resta sull'ultimo punto in cui era.
//!
//! In orizzontale l'inquadratura resta dentro il treno: non mostra mai oltre le
//! pareti di testata estreme (più un piccolo margine). Quando la camera è ferma
//! la sua posizione viene allineata alla griglia dei pixel dello schermo, così
//! testo e sprite restano nitidi senza introdurre scatti durante il movimento.

use bevy::{
    camera::{CameraProjection, ScalingMode},
    prelude::*,
    window::PrimaryWindow,
};

use sim::NpcId;

use crate::npc_render::{NpcSpriteIndex, npc_position};
use crate::player::Player;
use crate::sim_bridge::SimTickSet;
use crate::state::{FollowNpc, NpcSprite, SelectedNpc, Sim};
use crate::stations::StationLayout;
use crate::train::{FLOOR_Y, STOREY, TrainLayout, WALL};

/// Altezza visibile in unità mondo: con una finestra alta 720 px ogni unità
/// diventa esattamente 3 pixel, adatto alla pixel art.
const VIEWPORT_HEIGHT: f32 = 240.0;
/// Quanto sopra il giocatore guarda la camera, per inquadrare la carrozza.
const LOOK_UP: f32 = 30.0;
/// Velocità di inseguimento (più alto = più reattivo). In verticale è più lenta
/// così i salti non fanno sobbalzare l'inquadratura.
const DECAY_X: f32 = 6.0;
const DECAY_Y: f32 = 2.5;
/// Quanto cielo si vede oltre le pareti di testata alle estremità del treno.
const EDGE_MARGIN: f32 = 16.0;
/// Quota (sopra il pavimento) su cui si centra la camera che segue un NPC:
/// come per il giocatore in piedi, così seguire chi sale su un letto a
/// castello non fa muovere l'inquadratura in verticale.
const NPC_EYE_HEIGHT: f32 = 12.0;

/// Vista allargata (tasto Z, prototipo delle carrozze a due piani): mostra
/// entrambi i piani invece di seguire quello del giocatore.
const WIDE_VIEWPORT_HEIGHT: f32 = 300.0;
/// Quota su cui si centra la vista allargata: il mezzo di una carrozza a due piani.
const WIDE_CENTER_Y: f32 = 110.0;

/// Se la vista è allargata (tasto Z).
#[derive(Resource, Debug, Default)]
pub struct WideView(pub bool);

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        // `Update` gira dopo `RunFixedMainLoop`, quindi il Transform del
        // giocatore è già interpolato per questo frame.
        app.init_resource::<WideView>()
            .add_systems(Startup, spawn_camera)
            .add_systems(
                Update,
                (
                    update_follow,
                    toggle_wide_view,
                    apply_wide_view,
                    follow_target,
                )
                    .chain()
                    .after(SimTickSet),
            );
    }
}

/// Z alterna la vista che segue il piano del giocatore e quella allargata.
fn toggle_wide_view(keys: Res<ButtonInput<KeyCode>>, mut wide: ResMut<WideView>) {
    if keys.just_pressed(KeyCode::KeyZ) {
        wide.0 = !wide.0;
    }
}

fn apply_wide_view(wide: Res<WideView>, mut projection: Single<&mut Projection, With<Camera2d>>) {
    if !wide.is_changed() {
        return;
    }
    if let Projection::Orthographic(ortho) = &mut **projection {
        ortho.scaling_mode = ScalingMode::FixedVertical {
            viewport_height: if wide.0 {
                WIDE_VIEWPORT_HEIGHT
            } else {
                VIEWPORT_HEIGHT
            },
        };
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Projection::from(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: VIEWPORT_HEIGHT,
            },
            ..OrthographicProjection::default_2d()
        }),
    ));
}

/// Area visibile (in unità mondo, relativa alla camera) e dimensione di un
/// pixel fisico dello schermo in unità mondo.
///
/// La proiezione viene ricalcolata sulla finestra attuale invece di leggere
/// `area`, che Bevy aggiorna solo in `PostUpdate`: così è corretta anche al
/// primo frame e subito dopo un ridimensionamento.
fn view_metrics(ortho: &OrthographicProjection, window: &Window) -> Option<(Rect, f32)> {
    let size = window.physical_size().as_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        // Finestra minimizzata.
        return None;
    }
    let mut ortho = ortho.clone();
    ortho.update(size.x, size.y);
    Some((ortho.area, ortho.area.height() / size.y))
}

/// Limita la x della camera perché la vista (larga `2 * half_width`) non esca
/// da `[left - EDGE_MARGIN, right + EDGE_MARGIN]`. Se il treno è più stretto
/// della vista, lo centra.
fn clamp_to_train(x: f32, half_width: f32, left: f32, right: f32) -> f32 {
    let lo = left - EDGE_MARGIN + half_width;
    let hi = right + EDGE_MARGIN - half_width;
    if lo > hi {
        (left + right) / 2.0
    } else {
        x.clamp(lo, hi)
    }
}

/// Chi inquadra la camera.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CameraTarget {
    Player,
    Npc(NpcId),
}

/// La camera segue l'NPC selezionato solo con "Segui" attivo.
pub(crate) fn camera_target(follow: bool, selected: Option<NpcId>) -> CameraTarget {
    match selected {
        Some(id) if follow => CameraTarget::Npc(id),
        _ => CameraTarget::Player,
    }
}

/// Nuovo stato di "Segui": senza selezione è sempre spento; `toggle` (tasto F)
/// lo spegne, o lo accende se l'NPC selezionato è vivo.
pub(crate) fn next_follow(
    follow: bool,
    toggle: bool,
    selected: Option<NpcId>,
    alive: bool,
) -> bool {
    match selected {
        None => false,
        Some(_) if toggle => !follow && alive,
        Some(_) => follow,
    }
}

/// F: segue l'NPC selezionato o torna al giocatore.
fn update_follow(
    keys: Res<ButtonInput<KeyCode>>,
    sim: Res<Sim>,
    selected: Res<SelectedNpc>,
    mut follow: ResMut<FollowNpc>,
) {
    let alive = selected.0.is_some_and(|id| sim.world.npc(id).is_some());
    let next = next_follow(
        follow.0,
        keys.just_pressed(KeyCode::KeyF),
        selected.0,
        alive,
    );
    if follow.0 != next {
        follow.0 = next;
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn follow_target(
    time: Res<Time>,
    layout: Res<TrainLayout>,
    sim: Res<Sim>,
    stations: Res<StationLayout>,
    follow: Res<FollowNpc>,
    selected: Res<SelectedNpc>,
    index: Res<NpcSpriteIndex>,
    wide: Res<WideView>,
    window: Single<&Window, With<PrimaryWindow>>,
    player: Single<&Transform, (With<Player>, Without<Camera2d>)>,
    sprites: Query<&Transform, (With<NpcSprite>, Without<Camera2d>)>,
    camera: Single<(&mut Transform, &Projection), With<Camera2d>>,
    mut snapped: Local<bool>,
    mut last_npc: Local<Option<(NpcId, Vec2)>>,
) {
    let (mut camera, projection) = camera.into_inner();
    let Projection::Orthographic(ortho) = projection else {
        return;
    };
    let Some((view, pixel)) = view_metrics(ortho, &window) else {
        return;
    };

    let focus = match camera_target(follow.0, selected.0) {
        CameraTarget::Player => {
            *last_npc = None;
            Vec2::new(player.translation.x, player.translation.y + LOOK_UP)
        }
        CameraTarget::Npc(id) => {
            // Lo sprite se è disegnato (si muove in modo fluido), altrimenti
            // la posa calcolata dalla sim; se è morto, l'ultimo punto noto.
            let position = index
                .entity(id)
                .and_then(|e| sprites.get(e).ok())
                .map(|t| t.translation.truncate())
                .or_else(|| {
                    let npc = sim.world.npc(id)?;
                    Some(npc_position(&sim.world, &stations, npc))
                })
                .or_else(|| last_npc.filter(|&(last, _)| last == id).map(|(_, p)| p));
            match position {
                Some(p) => {
                    *last_npc = Some((id, p));
                    // All'altezza del suo piano, qualunque cosa faccia.
                    let floor = ((p.y - FLOOR_Y) / STOREY).floor().max(0.0);
                    Vec2::new(p.x, FLOOR_Y + floor * STOREY + NPC_EYE_HEIGHT + LOOK_UP)
                }
                None => Vec2::new(player.translation.x, player.translation.y + LOOK_UP),
            }
        }
    };

    // Estremi esterni del treno (facce esterne delle testate).
    let focus = if wide.0 {
        Vec2::new(focus.x, WIDE_CENTER_Y)
    } else {
        focus
    };
    let (inner_left, inner_right) = layout.inner_bounds();
    let target = Vec2::new(
        clamp_to_train(
            focus.x,
            view.half_size().x,
            inner_left - WALL,
            inner_right + WALL,
        ),
        focus.y,
    );
    let current = camera.translation.truncate();

    let next = if !*snapped {
        // Al primo frame la camera salta direttamente sull'obiettivo.
        *snapped = true;
        target
    } else if (target - current).abs().max_element() < pixel {
        // Ferma (a meno di un pixel dall'obiettivo): allineata alla griglia
        // dei pixel. Il valore è stabile finché l'obiettivo non si sposta.
        (target / pixel).round() * pixel
    } else {
        let dt = time.delta_secs();
        let mut next = current;
        next.x.smooth_nudge(&target.x, DECAY_X, dt);
        next.y.smooth_nudge(&target.y, DECAY_Y, dt);
        next
    };

    if next != current {
        camera.translation.x = next.x;
        camera.translation.y = next.y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_the_view_inside_the_train() {
        let half = 200.0;
        // Lontano dalle estremità: segue liberamente.
        assert_eq!(clamp_to_train(1000.0, half, 0.0, 5000.0), 1000.0);
        // All'estremità sinistra il bordo della vista si ferma al margine.
        let x = clamp_to_train(0.0, half, 0.0, 5000.0);
        assert_eq!(x - half, -EDGE_MARGIN);
        // All'estremità destra idem.
        let x = clamp_to_train(9000.0, half, 0.0, 5000.0);
        assert_eq!(x + half, 5000.0 + EDGE_MARGIN);
    }

    #[test]
    fn centers_a_train_narrower_than_the_view() {
        assert_eq!(clamp_to_train(-50.0, 400.0, 0.0, 320.0), 160.0);
        assert_eq!(clamp_to_train(900.0, 400.0, 0.0, 320.0), 160.0);
    }

    #[test]
    fn follows_the_selected_npc_only_when_asked() {
        let anna = NpcId(7);
        assert_eq!(camera_target(false, Some(anna)), CameraTarget::Player);
        assert_eq!(camera_target(true, Some(anna)), CameraTarget::Npc(anna));
        assert_eq!(camera_target(true, None), CameraTarget::Player);
    }

    #[test]
    fn follow_toggles_and_turns_off_without_selection() {
        let anna = Some(NpcId(7));
        // F accende (se è viva) e spegne.
        assert!(next_follow(false, true, anna, true));
        assert!(!next_follow(true, true, anna, true));
        // Non si comincia a seguire un morto, ma si può smettere.
        assert!(!next_follow(false, true, anna, false));
        assert!(!next_follow(true, true, anna, false));
        // Senza F non cambia (anche se l'NPC seguito è morto)...
        assert!(next_follow(true, false, anna, false));
        assert!(!next_follow(false, false, anna, true));
        // ...ma senza selezione si torna sempre al giocatore.
        assert!(!next_follow(true, false, None, false));
        assert!(!next_follow(false, true, None, false));
    }

    #[test]
    fn view_metrics_follow_the_window_aspect() {
        let ortho = OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical {
                viewport_height: VIEWPORT_HEIGHT,
            },
            ..OrthographicProjection::default_2d()
        };
        let window = Window {
            resolution: (1280, 720).into(),
            ..default()
        };
        let (view, pixel) = view_metrics(&ortho, &window).unwrap();
        assert!((view.height() - VIEWPORT_HEIGHT).abs() < 1e-3);
        assert!((view.width() - VIEWPORT_HEIGHT * 1280.0 / 720.0).abs() < 1e-3);
        assert!((pixel - VIEWPORT_HEIGHT / 720.0).abs() < 1e-6);
    }
}

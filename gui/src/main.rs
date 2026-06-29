use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(ImagePlugin::default_nearest()).set(WindowPlugin {
            primary_window: Some(Window {
                title: "Bipolar Orchestrator".to_string(),
                resolution: (1280_u32, 720_u32).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.04, 0.04, 0.10)))
        .add_systems(Startup, setup)
        .add_systems(Update, animate_sprites)
        .run();
}

#[derive(Component)]
struct AnimationTimer {
    timer: Timer,
    frame_count: usize,
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut layouts: ResMut<Assets<TextureAtlasLayout>>,
) {
    commands.spawn(Camera2d);

    let texture = asset_server.load("rustrelli.png");
    let layout = TextureAtlasLayout::from_grid(UVec2::new(48, 48), 8, 1, None, None);
    let layout_handle = layouts.add(layout);

    commands.spawn((
        Sprite {
            image: texture,
            texture_atlas: Some(TextureAtlas {
                layout: layout_handle,
                index: 0,
            }),
            custom_size: Some(Vec2::new(96.0, 96.0)),
            ..default()
        },
        Transform::from_xyz(0.0, 0.0, 0.0),
        AnimationTimer {
            timer: Timer::from_seconds(0.15, TimerMode::Repeating),
            frame_count: 8,
        },
    ));
}

fn animate_sprites(time: Res<Time>, mut query: Query<(&mut Sprite, &mut AnimationTimer)>) {
    for (mut sprite, mut anim) in &mut query {
        anim.timer.tick(time.delta());
        if anim.timer.just_finished() {
            if let Some(atlas) = &mut sprite.texture_atlas {
                atlas.index = (atlas.index + 1) % anim.frame_count;
            }
        }
    }
}

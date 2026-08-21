use std::sync::Arc;

use crate::command::CommandResult;
use crate::command::dispatcher::CommandError::InvalidConsumption;
use crate::command::{
    CommandExecutor, CommandSender,
    args::{
        Arg, ConsumedArgs, position_block::BlockPosArgumentConsumer,
        rotation::RotationArgumentConsumer,
    },
    dispatcher::CommandError,
    tree::{CommandTree, builder::argument},
};
use crate::net::ClientPlatform;
use crate::plugin::world::spawn_change::SpawnChangeEvent;
use crate::server::Server;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::translation;
use pumpkin_util::version::JavaMinecraftVersion;
use pumpkin_util::{math::position::BlockPos, text::TextComponent};

const NAMES: [&str; 1] = ["setworldspawn"];

const DESCRIPTION: &str = "Sets the world spawn point.";

const ARG_BLOCK_POS: &str = "position";

const ARG_ANGLE: &str = "angle";

struct NoArgsWorldSpawnExecutor;

impl CommandExecutor for NoArgsWorldSpawnExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        server: &'a crate::server::Server,
        _args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(player) = sender.as_player() else {
                if sender.is_console() {
                    return Err(CommandError::CommandFailed(TextComponent::text(
                        "You must specify a Position!",
                    )));
                }
                return Err(CommandError::CommandFailed(TextComponent::text(
                    "Failed to get Sender as Player!",
                )));
            };
            let block_pos = player.position();
            setworldspawn(sender, server, block_pos.to_block_pos(), 0.0, 0.0).await
        })
    }
}

struct DefaultWorldSpawnExecutor;

impl CommandExecutor for DefaultWorldSpawnExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        server: &'a crate::server::Server,
        args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(Arg::BlockPos(block_pos)) = args.get(ARG_BLOCK_POS) else {
                return Err(InvalidConsumption(Some(ARG_BLOCK_POS.into())));
            };

            setworldspawn(sender, server, *block_pos, 0.0, 0.0).await
        })
    }
}

struct AngleWorldSpawnExecutor;

impl CommandExecutor for AngleWorldSpawnExecutor {
    fn execute<'a>(
        &'a self,
        sender: &'a CommandSender,
        server: &'a crate::server::Server,
        args: &'a ConsumedArgs<'a>,
    ) -> CommandResult<'a> {
        Box::pin(async move {
            let Some(Arg::BlockPos(block_pos)) = args.get(ARG_BLOCK_POS) else {
                return Err(InvalidConsumption(Some(ARG_BLOCK_POS.into())));
            };

            // Note: Rotation argument is (yaw, is_yaw_relative, pitch, is_pitch_relative)
            // For setworldspawn, we use absolute values only (ignore relative flags)
            let Some(Arg::Rotation(yaw, _, pitch, _)) = args.get(ARG_ANGLE) else {
                return Err(InvalidConsumption(Some(ARG_ANGLE.into())));
            };

            setworldspawn(sender, server, *block_pos, *yaw, *pitch).await
        })
    }
}

async fn setworldspawn(
    sender: &CommandSender,
    server: &Server,
    block_pos: BlockPos,
    yaw: f32,
    pitch: f32,
) -> Result<i32, CommandError> {
    let Some(world) = sender.world_or_first(server) else {
        return Err(CommandError::CommandFailed(TextComponent::text(
            "Failed to get world.",
        )));
    };
    if world.dimension != Dimension::OVERWORLD && world.dimension != Dimension::OVERWORLD_CAVES {
        return Err(CommandError::CommandFailed(TextComponent::translate_cross(
            translation::java::COMMANDS_SETWORLDSPAWN_FAILURE_NOT_OVERWORLD,
            translation::java::COMMANDS_SETWORLDSPAWN_FAILURE_NOT_OVERWORLD,
            [],
        )));
    }

    let current_info = server.level_info.load();
    let previous_position = BlockPos::new(
        current_info.spawn_x,
        current_info.spawn_y,
        current_info.spawn_z,
    );
    let mut new_position = block_pos;
    let previous_yaw = current_info.spawn_yaw;
    let previous_pitch = current_info.spawn_pitch;
    let mut new_yaw = yaw;
    let mut new_pitch = pitch;
    let mut event = SpawnChangeEvent::new(
        world.clone(),
        previous_position,
        previous_yaw,
        previous_pitch,
        new_position,
        new_yaw,
        new_pitch,
    );
    if let Some(server_arc) = world.server.upgrade() {
        server_arc
            .plugin_manager
            .fire(&server_arc, &mut event)
            .await;
    }
    new_position = event.new_position;
    new_yaw = event.new_yaw;
    new_pitch = event.new_pitch;

    let mut new_info = (**current_info).clone();

    new_info.spawn_x = new_position.0.x;
    new_info.spawn_y = new_position.0.y;
    new_info.spawn_z = new_position.0.z;
    new_info.spawn_yaw = new_yaw;
    new_info.spawn_pitch = new_pitch;

    server.level_info.store(Arc::new(new_info));

    let client_version = sender.as_player().and_then(|player| {
        if let ClientPlatform::Java(client) = player.client.as_ref() {
            Some(client.version.load())
        } else {
            None
        }
    });

    let message = success_message(
        client_version,
        new_position,
        new_yaw,
        new_pitch,
        world.dimension.minecraft_name,
    );

    sender.send_message(message).await;

    Ok(1)
}

/// `commands.setworldspawn.success.new` (with pitch and dimension) was only added in 26.1;
/// clients on 1.21.11 and earlier only know the older `commands.setworldspawn.success`
/// (position and yaw only), and show the raw key untranslated if we send them the new one.
/// `client_version` is `None` for senders with no Java protocol version to gate on (console,
/// RCON, Bedrock players), which always get the current message.
fn success_message(
    client_version: Option<JavaMinecraftVersion>,
    position: BlockPos,
    yaw: f32,
    pitch: f32,
    dimension_name: &'static str,
) -> TextComponent {
    let supports_new_message = client_version.is_none_or(|v| v >= JavaMinecraftVersion::V_26_1);

    if supports_new_message {
        TextComponent::translate_cross(
            translation::java::COMMANDS_SETWORLDSPAWN_SUCCESS_NEW,
            translation::java::COMMANDS_SETWORLDSPAWN_SUCCESS_NEW,
            [
                TextComponent::text(position.0.x.to_string()),
                TextComponent::text(position.0.y.to_string()),
                TextComponent::text(position.0.z.to_string()),
                TextComponent::text(yaw.to_string()),
                TextComponent::text(pitch.to_string()),
                TextComponent::text(dimension_name),
            ],
        )
    } else {
        TextComponent::translate_cross(
            translation::java::COMMANDS_SETWORLDSPAWN_SUCCESS,
            translation::java::COMMANDS_SETWORLDSPAWN_SUCCESS,
            [
                TextComponent::text(position.0.x.to_string()),
                TextComponent::text(position.0.y.to_string()),
                TextComponent::text(position.0.z.to_string()),
                TextComponent::text(yaw.to_string()),
            ],
        )
    }
}

#[must_use]
pub fn init_command_tree() -> CommandTree {
    CommandTree::new(NAMES, DESCRIPTION)
        .execute(NoArgsWorldSpawnExecutor)
        .then(
            argument(ARG_BLOCK_POS, BlockPosArgumentConsumer)
                .execute(DefaultWorldSpawnExecutor)
                .then(
                    argument(ARG_ANGLE, RotationArgumentConsumer).execute(AngleWorldSpawnExecutor),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::text::TextContent;

    fn with_count(component: &TextComponent) -> (&str, usize) {
        match &*component.0.content {
            TextContent::Translate { translate, with, .. } => (translate.as_ref(), with.len()),
            _ => panic!("expected a Translate component"),
        }
    }

    #[test]
    fn pre_26_1_client_gets_the_four_argument_legacy_key() {
        let message = success_message(
            Some(JavaMinecraftVersion::V_1_21_11),
            BlockPos::new(1, 2, 3),
            90.0,
            0.0,
            "minecraft:overworld",
        );
        let (key, arg_count) = with_count(&message);
        assert_eq!(key, translation::java::COMMANDS_SETWORLDSPAWN_SUCCESS);
        assert_eq!(arg_count, 4);
    }

    #[test]
    fn v26_1_and_later_client_gets_the_six_argument_new_key() {
        for version in [JavaMinecraftVersion::V_26_1, JavaMinecraftVersion::V_26_2] {
            let message = success_message(
                Some(version),
                BlockPos::new(1, 2, 3),
                90.0,
                0.0,
                "minecraft:overworld",
            );
            let (key, arg_count) = with_count(&message);
            assert_eq!(key, translation::java::COMMANDS_SETWORLDSPAWN_SUCCESS_NEW);
            assert_eq!(arg_count, 6);
        }
    }

    #[test]
    fn console_and_non_java_senders_get_the_current_message() {
        let message = success_message(
            None,
            BlockPos::new(1, 2, 3),
            90.0,
            0.0,
            "minecraft:overworld",
        );
        let (key, arg_count) = with_count(&message);
        assert_eq!(key, translation::java::COMMANDS_SETWORLDSPAWN_SUCCESS_NEW);
        assert_eq!(arg_count, 6);
    }
}

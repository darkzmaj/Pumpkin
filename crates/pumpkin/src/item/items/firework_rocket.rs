use std::pin::Pin;
use std::sync::Arc;

use crate::entity::player::Player;
use crate::entity::projectile::firework_rocket::FireworkRocketEntity;
use crate::entity::{Entity, EntityBase};
use crate::item::{ItemBehaviour, ItemMetadata};
use crate::server::Server;
use pumpkin_data::Block;
use pumpkin_data::BlockDirection;
use pumpkin_data::entity::EntityType;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;

pub struct FireworkRocketItem;

impl ItemMetadata for FireworkRocketItem {
    fn ids() -> Box<[u16]> {
        [Item::FIREWORK_ROCKET.id].into()
    }
}

impl ItemBehaviour for FireworkRocketItem {
    fn use_on_block<'a>(
        &'a self,
        _item: &'a mut ItemStack,
        player: &'a Player,
        _location: BlockPos,
        _face: BlockDirection,
        _cursor_pos: Vector3<f32>,
        _block: &'a Block,
        _server: &'a Server,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        // Firework rockets aren't placeable; aiming at a block while gliding must still trigger
        // the elytra boost, same as `normal_use` does when aiming at open air.
        Box::pin(async move {
            if player.get_entity().is_fall_flying() {
                let world = player.world();
                let entity = Entity::new(
                    world.clone(),
                    player.get_entity().pos.load(),
                    &EntityType::FIREWORK_ROCKET,
                );
                let entity = FireworkRocketEntity::new_shot(entity, player.get_entity());
                world.spawn_entity(Arc::new(entity)).await;
            }
        })
    }

    fn normal_use<'a>(
        &'a self,
        _item: &'a Item,
        player: &'a Player,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {
            if player.get_entity().is_fall_flying() {
                let world = player.world();
                let entity = Entity::new(
                    world.clone(),
                    player.get_entity().pos.load(),
                    &EntityType::FIREWORK_ROCKET,
                );
                let entity = FireworkRocketEntity::new_shot(entity, player.get_entity());
                world.spawn_entity(Arc::new(entity)).await;
            }
        })
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

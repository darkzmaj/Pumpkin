use crate::VarInt;
use crate::codec::data_component::{deserialize, serialize};
use crate::ser::{NetworkReadExt, NetworkWriteExt, ReadingError, WritingError};
use pumpkin_data::data_component::DataComponent;
use pumpkin_data::data_component_impl::{CustomNameImpl, DataComponentImpl, ItemNameImpl};
use pumpkin_data::item::Item;
use pumpkin_data::item_id_remap::{remap_item_id_for_version, remap_item_id_from_version};
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::text::TextComponent;
use pumpkin_util::version::JavaMinecraftVersion;
use std::borrow::Cow;
use std::io::Cursor;

#[derive(Clone)]
pub struct ItemStackSerializer<'a>(pub Cow<'a, ItemStack>);

fn item_component_counts(stack: &ItemStack) -> (u8, u8) {
    let mut to_add = 0u8;
    let mut to_remove = 0u8;

    for (_id, data) in &stack.patch {
        if data.is_none() {
            to_remove += 1;
        } else {
            to_add += 1;
        }
    }

    (to_add, to_remove)
}

/// The wire id for a data component under the connecting client's protocol version. See
/// `read_component_id`'s comment - the `minecraft:data_component_type` registry isn't stable
/// across versions, so a component sent to a pre-26.1 client must use its legacy id, not the
/// current (26.1+) one `DataComponent::to_id` returns.
fn component_wire_id(id: DataComponent, version: JavaMinecraftVersion) -> u8 {
    if version < JavaMinecraftVersion::V_26_1 {
        id.to_id_legacy().unwrap_or_else(|| id.to_id())
    } else {
        id.to_id()
    }
}

fn serialize_any_item_stack_with_id(
    stack: &ItemStack,
    item_id: u16,
    is_template: bool,
    write: &mut impl NetworkWriteExt,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    if stack.is_empty() {
        write.put_var_int(&VarInt(0))
    } else {
        let (to_add, to_remove) = item_component_counts(stack);
        if is_template {
            write.put_var_int(&VarInt::from(item_id))?;
            write.put_var_int(&VarInt::from(stack.item_count))?;
        } else {
            write.put_var_int(&VarInt::from(stack.item_count))?;
            write.put_var_int(&VarInt::from(item_id))?;
        }
        write.put_var_int(&VarInt::from(to_add))?;
        write.put_var_int(&VarInt::from(to_remove))?;

        for (id, data) in &stack.patch {
            if let Some(data) = data {
                write.put_var_int(&VarInt::from(component_wire_id(*id, version)))?;
                serialize(*id, data.as_ref(), write)?;
            }
        }

        for (id, data) in &stack.patch {
            if data.is_none() {
                write.put_var_int(&VarInt::from(component_wire_id(*id, version)))?;
            }
        }

        Ok(())
    }
}

fn serialize_item_stack_with_id(
    stack: &ItemStack,
    item_id: u16,
    write: &mut impl NetworkWriteExt,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    serialize_any_item_stack_with_id(stack, item_id, false, write, version)
}

fn serialize_length_prefixed_item_stack_with_id(
    stack: &ItemStack,
    item_id: u16,
    write: &mut impl NetworkWriteExt,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    if stack.is_empty() {
        write.put_var_int(&VarInt(0))
    } else {
        let (to_add, to_remove) = item_component_counts(stack);
        write.put_var_int(&VarInt::from(stack.item_count))?;
        write.put_var_int(&VarInt::from(item_id))?;
        write.put_var_int(&VarInt::from(to_add))?;
        write.put_var_int(&VarInt::from(to_remove))?;

        for (id, data) in &stack.patch {
            if let Some(data) = data {
                write.put_var_int(&VarInt::from(component_wire_id(*id, version)))?;
                let mut comp_buf = Vec::new();
                serialize(*id, data.as_ref(), &mut comp_buf)?;
                write.put_var_int(&VarInt::from(comp_buf.len() as i32))?;
                write.write_slice(&comp_buf)?;
            }
        }

        for (id, data) in &stack.patch {
            if data.is_none() {
                write.put_var_int(&VarInt::from(component_wire_id(*id, version)))?;
            }
        }

        Ok(())
    }
}

fn serialize_item_cost_with_id(
    stack: &ItemStack,
    item_id: u16,
    write: &mut impl NetworkWriteExt,
    version: JavaMinecraftVersion,
) -> Result<(), WritingError> {
    let component_count = stack
        .patch
        .iter()
        .filter(|(_, data)| data.is_some())
        .count();
    let component_count = i32::try_from(component_count)
        .map_err(|_| WritingError::Message("Too many item cost components".into()))?;

    write.put_var_int(&VarInt::from(item_id))?;
    write.put_var_int(&VarInt::from(stack.item_count))?;
    write.put_var_int(&VarInt(component_count))?;
    for (id, data) in &stack.patch {
        if let Some(data) = data {
            write.put_var_int(&VarInt::from(component_wire_id(*id, version)))?;
            serialize(*id, data.as_ref(), write)?;
        }
    }
    Ok(())
}

fn read_component_id(
    read: &mut impl NetworkReadExt,
    version: JavaMinecraftVersion,
) -> Result<DataComponent, ReadingError> {
    let id_val = read.get_var_int()?.0;
    let id_u8 = id_val
        .try_into()
        .map_err(|_| ReadingError::Message(format!("Invalid component ID: {id_val}")))?;
    // The `minecraft:data_component_type` registry's wire ids are not stable across protocol
    // versions: components added since the last pre-26.1 release shift every id after their
    // insertion point, so an id sent by an older client must be resolved through the legacy
    // table instead of the (26.1+) table `try_from_id` uses.
    let component = if version < JavaMinecraftVersion::V_26_1 {
        DataComponent::try_from_id_legacy(id_u8)
    } else {
        DataComponent::try_from_id(id_u8)
    };
    component.ok_or_else(|| ReadingError::Message(format!("Unknown component ID: {id_val}")))
}

fn decode_custom_name(component_data: &[u8]) -> Result<Box<dyn DataComponentImpl>, ReadingError> {
    let mut cursor = Cursor::new(component_data);
    let mut nbt_reader = pumpkin_nbt::deserializer::NbtReadHelperJava::new(&mut cursor);
    let tag = NbtTag::deserialize(&mut nbt_reader)
        .map_err(|err| ReadingError::Message(format!("Failed to decode CustomName NBT: {err}")))?;
    let name = match tag {
        NbtTag::String(name) => TextComponent::text(name.to_string()),
        NbtTag::Compound(compound) => compound
            .get_string("text")
            .map_or_else(TextComponent::empty, |name| {
                TextComponent::text(name.to_string())
            }),
        _ => TextComponent::empty(),
    };
    Ok(CustomNameImpl { name }.to_dyn())
}

fn decode_item_name(component_data: &[u8]) -> Result<Box<dyn DataComponentImpl>, ReadingError> {
    let mut cursor = Cursor::new(component_data);
    let mut nbt_reader = pumpkin_nbt::deserializer::NbtReadHelperJava::new(&mut cursor);
    let tag = NbtTag::deserialize(&mut nbt_reader)
        .map_err(|err| ReadingError::Message(format!("Failed to decode ItemName NBT: {err}")))?;
    let name = match tag {
        NbtTag::String(name) => name.to_string(),
        NbtTag::Compound(compound) => compound
            .get_string("translate")
            .or_else(|| compound.get_string("text"))
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    };
    Ok(ItemNameImpl {
        name: Cow::Owned(name),
    }
    .to_dyn())
}

fn decode_component(
    id: DataComponent,
    component_data: &[u8],
) -> Result<Box<dyn DataComponentImpl>, ReadingError> {
    match id {
        DataComponent::CustomName => decode_custom_name(component_data),
        DataComponent::ItemName => decode_item_name(component_data),
        _ => {
            let mut cursor = Cursor::new(component_data);
            deserialize(id, &mut cursor)
        }
    }
}

fn read_length_prefixed_component(
    read: &mut impl NetworkReadExt,
    version: JavaMinecraftVersion,
) -> Result<(DataComponent, Box<dyn DataComponentImpl>), ReadingError> {
    let id = read_component_id(read, version)?;
    let byte_len = read.get_var_int()?.0;
    let byte_len: usize = byte_len
        .try_into()
        .map_err(|_| ReadingError::Message("Negative component data length".into()))?;
    if byte_len > crate::MAX_PACKET_DATA_SIZE {
        return Err(ReadingError::TooLarge("Component data too large".into()));
    }

    let component_impl = if byte_len <= 256 {
        let mut stack_buf = [0u8; 256];
        let slice = &mut stack_buf[..byte_len];
        read.read_bytes_to_buf(slice)?;
        decode_component(id, slice)?
    } else {
        let mut component_data = vec![0u8; byte_len];
        read.read_bytes_to_buf(&mut component_data)?;
        decode_component(id, &component_data)?
    };

    Ok((id, component_impl))
}

impl ItemStackSerializer<'_> {
    pub fn read(
        read: &mut impl NetworkReadExt,
        version: &JavaMinecraftVersion,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        const MAX_COMPONENTS: i32 = 256;

        let item_count = read.get_var_int()?;
        if item_count.0 == 0 {
            return Ok(ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)));
        }

        let item_id = read.get_var_int()?;
        let num_to_add = read.get_var_int()?.0;
        let num_to_remove = read.get_var_int()?.0;

        if num_to_add < 0 || num_to_remove < 0 {
            return Err(ReadingError::Message("Negative component count".into()));
        }

        let total_components = num_to_add
            .checked_add(num_to_remove)
            .ok_or_else(|| ReadingError::Message("Component count overflow".into()))?;

        if total_components > MAX_COMPONENTS {
            return Err(ReadingError::Message(
                "Too many components in ItemStack patch".into(),
            ));
        }

        let mut patch = Vec::with_capacity((num_to_add + num_to_remove) as usize);

        for _ in 0..num_to_add {
            let id = read_component_id(read, *version)?;

            // The plain Slot format has no per-component byte-length prefix (that's only used
            // by the length-prefixed variant, e.g. Set Creative Mode Slot); reading one here
            // would desync the rest of the stream.
            let component_impl = deserialize(id, read)?;
            patch.push((id, Some(component_impl)));
        }

        for _ in 0..num_to_remove {
            patch.push((read_component_id(read, *version)?, None));
        }

        let item_id_u16: u16 = item_id
            .0
            .try_into()
            .map_err(|_| ReadingError::Message("Invalid item id!".into()))?;

        Ok(ItemStackSerializer(Cow::Owned(
            ItemStack::new_with_component(
                item_count.0 as u8,
                Item::from_id(item_id_u16).unwrap_or(&Item::AIR),
                patch,
            ),
        )))
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        // No client version is available here (this is the unversioned path used by entity
        // metadata and other callers without a specific recipient in scope); fall back to the
        // current wire ids, matching this function's behavior before per-version resolution
        // existed. See `write_with_version` for the version-aware path.
        serialize_item_stack_with_id(self.0.as_ref(), self.0.item.id, write, CURRENT_MC_VERSION)
    }

    pub fn read_length_prefixed_optional(
        read: &mut impl NetworkReadExt,
        version: &JavaMinecraftVersion,
    ) -> Result<ItemStackSerializer<'static>, ReadingError> {
        const MAX_COMPONENTS: i32 = 256;

        let item_count = read.get_var_int()?;
        if item_count.0 == 0 {
            return Ok(ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)));
        }
        let item_count_u8 = item_count
            .0
            .try_into()
            .map_err(|_| ReadingError::Message("Invalid item count!".into()))?;

        let item_id = read.get_var_int()?;
        let num_to_add = read.get_var_int()?.0;
        let num_to_remove = read.get_var_int()?.0;

        if num_to_add < 0 || num_to_remove < 0 {
            return Err(ReadingError::Message("Negative component count".into()));
        }

        let total_components = num_to_add
            .checked_add(num_to_remove)
            .ok_or_else(|| ReadingError::Message("Component count overflow".into()))?;

        if total_components > MAX_COMPONENTS {
            return Err(ReadingError::Message(
                "Too many components in ItemStack patch".into(),
            ));
        }

        let mut patch = Vec::with_capacity(total_components as usize);

        for _ in 0..num_to_add {
            let (id, component_impl) = read_length_prefixed_component(read, *version)?;
            patch.push((id, Some(component_impl)));
        }

        for _ in 0..num_to_remove {
            patch.push((read_component_id(read, *version)?, None));
        }

        let item_id_u16 = item_id
            .0
            .try_into()
            .map_err(|_| ReadingError::Message("Invalid item id!".into()))?;

        Ok(ItemStackSerializer(Cow::Owned(
            ItemStack::new_with_component(
                item_count_u8,
                Item::from_id(item_id_u16).unwrap_or(&Item::AIR),
                patch,
            ),
        )))
    }

    pub fn write_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let remapped_item_id = remap_item_id_for_version(self.0.item.id, *version);
        serialize_item_stack_with_id(self.0.as_ref(), remapped_item_id, write, *version)
    }

    pub fn write_length_prefixed_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let remapped_item_id = remap_item_id_for_version(self.0.item.id, *version);
        serialize_length_prefixed_item_stack_with_id(
            self.0.as_ref(),
            remapped_item_id,
            write,
            *version,
        )
    }

    pub fn write_item_cost_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let remapped_item_id = remap_item_id_for_version(self.0.item.id, *version);
        serialize_item_cost_with_id(self.0.as_ref(), remapped_item_id, write, *version)
    }

    #[must_use]
    pub fn to_stack(self) -> ItemStack {
        self.0.into_owned()
    }

    #[must_use]
    pub fn to_stack_for_version(self, version: &JavaMinecraftVersion) -> ItemStack {
        let mut stack = self.0.into_owned();
        if stack.is_empty() {
            return stack;
        }

        let remapped_item_id = remap_item_id_from_version(stack.item.id, *version);
        stack.item = Item::from_id(remapped_item_id).unwrap_or(&Item::AIR);
        stack
    }
}

impl From<ItemStack> for ItemStackSerializer<'_> {
    fn from(item: ItemStack) -> Self {
        ItemStackSerializer(Cow::Owned(item))
    }
}

impl From<Option<ItemStack>> for ItemStackSerializer<'_> {
    fn from(item: Option<ItemStack>) -> Self {
        item.map_or_else(
            || ItemStackSerializer(Cow::Borrowed(ItemStack::EMPTY)),
            ItemStackSerializer::from,
        )
    }
}

#[derive(Debug, Clone)]
pub struct ItemComponentHash {
    pub added: Vec<(VarInt, i32)>,
    pub removed: Vec<VarInt>,
}

impl ItemComponentHash {
    pub fn read(read: &mut impl NetworkReadExt) -> Result<Self, ReadingError> {
        const MAX_COMPONENTS: i32 = 256;

        let added_length = read.get_var_int()?;
        if added_length.0 < 0 || added_length.0 > MAX_COMPONENTS {
            return Err(ReadingError::Message("added_length out of bounds".into()));
        }
        let mut added = Vec::with_capacity(added_length.0 as usize);
        for _ in 0..added_length.0 {
            let component_id = read.get_var_int()?;
            let component_value = read.get_i32()?;
            added.push((component_id, component_value));
        }

        let removed_length = read.get_var_int()?;
        if removed_length.0 < 0 || removed_length.0 > MAX_COMPONENTS {
            return Err(ReadingError::Message("removed_length out of bounds".into()));
        }
        let mut removed = Vec::with_capacity(removed_length.0 as usize);
        for _ in 0..removed_length.0 {
            let component_id = read.get_var_int()?;
            removed.push(component_id);
        }

        Ok(Self { added, removed })
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        write.put_var_int(&VarInt::from(self.added.len() as i32))?;
        for (id, val) in &self.added {
            write.put_var_int(id)?;
            write.put_i32(*val)?;
        }
        write.put_var_int(&VarInt::from(self.removed.len() as i32))?;
        for id in &self.removed {
            write.put_var_int(id)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ItemStackHash {
    item_id: VarInt,
    count: VarInt,
    components: ItemComponentHash,
}

#[derive(Debug, Clone)]
pub struct OptionalItemStackHash(pub Option<ItemStackHash>);

impl OptionalItemStackHash {
    pub fn read(read: &mut impl NetworkReadExt) -> Result<Self, ReadingError> {
        let is_some = read.get_bool()?;
        if is_some {
            let item_id = read.get_var_int()?;
            let count = read.get_var_int()?;
            let components = ItemComponentHash::read(read)?;

            Ok(Self(Some(ItemStackHash {
                item_id,
                count,
                components,
            })))
        } else {
            Ok(Self(None))
        }
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        if let Some(hash) = &self.0 {
            write.put_bool(true)?;
            write.put_var_int(&hash.item_id)?;
            write.put_var_int(&hash.count)?;
            hash.components.write(write)?;
        } else {
            write.put_bool(false)?;
        }
        Ok(())
    }

    #[must_use]
    pub fn hash_equals(&self, other: &ItemStack) -> bool {
        if let Some(hash) = &self.0 {
            if hash.item_id != other.item.id.into() || hash.count != other.item_count.into() {
                return false;
            }
            let calc = || {
                let mut to_add = 0u8;
                let mut to_remove = 0u8;
                for (_id, data) in &other.patch {
                    if data.is_none() {
                        to_remove += 1;
                    } else {
                        to_add += 1;
                    }
                }
                (to_add, to_remove)
            };
            let (to_add, to_remove) = calc();
            if to_add as usize != hash.components.added.len()
                || to_remove as usize != hash.components.removed.len()
            {
                return false;
            }
            for (other_id, data) in &other.patch {
                if let Some(data) = data {
                    let checksum = data.get_hash();
                    for (id, hash) in &hash.components.added {
                        if id == &VarInt::from(other_id.to_id()) {
                            if hash == &checksum {
                                break;
                            }
                            return false;
                        }
                    }
                } else if !hash
                    .components
                    .removed
                    .contains(&VarInt::from(other_id.to_id()))
                {
                    return false;
                }
            }
            true
        } else {
            other.is_empty()
        }
    }
}

pub struct ItemStackTemplateSerializer<'a>(pub Cow<'a, ItemStack>);

impl ItemStackTemplateSerializer<'_> {
    pub fn write_with_version(
        &self,
        write: &mut impl NetworkWriteExt,
        version: &JavaMinecraftVersion,
    ) -> Result<(), WritingError> {
        let remapped_item_id = remap_item_id_for_version(self.0.item.id, *version);
        serialize_any_item_stack_with_id(
            self.0.as_ref(),
            remapped_item_id,
            *version >= JavaMinecraftVersion::V_26_1,
            write,
            *version,
        )
    }

    pub fn write(&self, write: &mut impl NetworkWriteExt) -> Result<(), WritingError> {
        serialize_any_item_stack_with_id(
            self.0.as_ref(),
            self.0.item.id,
            true,
            write,
            CURRENT_MC_VERSION,
        )
    }
}

impl From<ItemStack> for ItemStackTemplateSerializer<'_> {
    fn from(item: ItemStack) -> Self {
        ItemStackTemplateSerializer(Cow::Owned(item))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_data::data_component_impl::FireworksImpl;

    // Regression test for a real 1.21.11 client's Set Creative Mode Slot packet: a plain
    // firework rocket (flight_duration=3, no explosions) was misdecoded because component wire
    // ids shifted between the pre-26.1 and 26.1+ `minecraft:data_component_type` registries -
    // id 67 means `fireworks` on 1.21.11 but `lodestone_tracker` on 26.1+, and Pumpkin's id
    // table only tracked the newer numbering until the legacy table was added.
    #[test]
    fn legacy_client_firework_component_id_resolves_correctly() {
        // Captured bytes after the leading 2-byte slot field: item_count, item_id, num_to_add,
        // num_to_remove, then one added component (id=67, len=2, data=[flight_duration=3,
        // explosions_len=0]).
        let mut bytes: &[u8] = &[0x40, 0xda, 0x09, 0x01, 0x00, 0x43, 0x02, 0x03, 0x00];

        let stack = ItemStackSerializer::read_length_prefixed_optional(
            &mut bytes,
            &JavaMinecraftVersion::V_1_21_11,
        )
        .expect("legacy client's firework component should decode")
        .to_stack();

        let fireworks = stack
            .patch
            .iter()
            .find(|(id, _)| *id == DataComponent::Fireworks)
            .and_then(|(_, data)| data.as_ref())
            .and_then(|data| data.as_any().downcast_ref::<FireworksImpl>())
            .expect("component id 67 resolved to Fireworks, not LodestoneTracker");
        assert_eq!(fireworks.flight_duration, 3);
        assert!(fireworks.explosions.is_empty());
    }

    // Regression test for the write-direction counterpart of the bug above: sending a saved
    // inventory (e.g. via Set Container Content on join) back to a pre-26.1 client must also use
    // the legacy component id, or the client's own decoder rejects the packet outright
    // ("Failed to decode packet 'clientbound/minecraft:container_set_content'").
    #[test]
    fn legacy_client_receives_legacy_fireworks_component_id() {
        let fireworks = FireworksImpl::new(3, Vec::new());
        let mut stack = ItemStack::new(64, &Item::FIREWORK_ROCKET);
        stack
            .patch
            .push((DataComponent::Fireworks, Some(fireworks.to_dyn())));
        let serializer = ItemStackSerializer::from(stack);

        let mut bytes = Vec::new();
        serializer
            .write_with_version(&mut bytes, &JavaMinecraftVersion::V_1_21_11)
            .expect("serialize for a legacy client");

        // Skip item_count, item_id, num_to_add, num_to_remove; the next byte is the component id.
        let mut cursor = std::io::Cursor::new(bytes.as_slice());
        cursor.get_var_int().unwrap();
        cursor.get_var_int().unwrap();
        cursor.get_var_int().unwrap();
        cursor.get_var_int().unwrap();
        let component_id = bytes[cursor.position() as usize];
        assert_eq!(
            component_id, 67,
            "fireworks must be written as legacy id 67 for a 1.21.11 client, not the current id 69"
        );
    }
}

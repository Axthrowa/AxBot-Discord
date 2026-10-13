//! Discord form pencereleri (modal) için ortak yardımcılar.

use poise::serenity_prelude::{
    ActionRowComponent, CreateActionRow, CreateInputText, InputTextStyle, ModalInteraction,
};

/// Discord'un bir formdaki en fazla metin kutusu sayısı.
pub const MAX_FIELDS: usize = 5;
/// Metin kutusu başlığı en fazla 45 karakter olabilir.
pub const MAX_LABEL: usize = 45;

pub fn text_field(custom_id: &str, label: &str, paragraph: bool, required: bool, max_length: u16) -> CreateActionRow {
    let style = if paragraph { InputTextStyle::Paragraph } else { InputTextStyle::Short };
    let label: String = label.chars().take(MAX_LABEL).collect();
    CreateActionRow::InputText(
        CreateInputText::new(style, label, custom_id)
            .required(required)
            .max_length(max_length),
    )
}

/// Formdaki kutuların değerleri (sırasıyla).
pub fn values(modal: &ModalInteraction) -> Vec<String> {
    modal
        .data
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .filter_map(|c| match c {
            ActionRowComponent::InputText(input) => Some(input.value.clone().unwrap_or_default()),
            _ => None,
        })
        .collect()
}

//! The game's own master files (`LocalOptionData` next to `hkb.exe`, the client's local copies of
//! the server's lists): the shops' products, the cards a reward draws from, the guest's profile
//! a new player starts with.

use wal_payload_common::json::Json;

/// A file of the game's `LocalOptionData` (UTF-8, its BOM dropped).
pub(super) fn read(path: &str) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let text = std::fs::read_to_string(exe.parent()?.join("LocalOptionData").join(path)).ok()?;
    Some(text.trim_start_matches('\u{feff}').to_string())
}

fn read_json(path: &str) -> Json {
    read(path).and_then(|t| Json::parse(&t)).unwrap_or(Json::obj())
}

/// The lists' version: the client reloads a list whose number changed (its own starts at 0).
const UPDATE_NUM: i64 = 1;

fn products(update_num: i64, items: Vec<Json>) -> Json {
    // the client's list already this one: no products sent again
    let items = if update_num == UPDATE_NUM { Vec::new() } else { items };
    Json::obj().with("updateNum", UPDATE_NUM).with("productInfo", items)
}

/// The summon assist ticket (`TicketData.json`'s 101: five make a summon).
pub(super) const SUMMON_TICKET: i64 = 101;

/// `get_product_shop_card`: the card summons (the game's own lists come from the server): one
/// card for a credit, three for two (the cabinet's credits: free in free play), one for five
/// summon assist tickets (the CP a play leaves, made tickets at logout).
pub(super) fn shop_card(update_num: i64) -> Json {
    let product = |id: i64, name: &str, description: &str, consume_type: i64, ticket: i64, costs: [(i64, i64); 2]| {
        Json::obj()
            .with("productId", id)
            // always on sale
            .with("productType", 1u32)
            .with("productName", name)
            .with("productDescription", description)
            .with("startDate", 1_514_757_600_000i64)
            .with("endDate", 4_102_412_400_000i64)
            .with("rootProductId", id)
            .with("nextProductId", 0u32)
            // 1: credits, 2: tickets
            .with("consumeType", consume_type)
            .with("ticketId", ticket)
            .with("consumeNum1", costs[0].0)
            .with("publishNum1", costs[0].1)
            .with("consumeNum2", costs[1].0)
            .with("publishNum2", costs[1].1)
            .with("consumeNum3", 0u32)
            .with("publishNum3", 0u32)
            .with("limitDailyNum", 0u32)
            .with("limitTotalNum", 0u32)
            .with("imageFile1", "SHO_banner_gacha_001")
            .with("imageFile2", "SHO_banner_gacha_001")
            .with("assuranceRarityInfo", Vec::<Json>::new())
            .with("viewPriority", id.to_string())
    };
    products(
        update_num,
        vec![
            product(1, "カード召喚", "クレジットでカードを召喚します。", 1, 0, [(1, 1), (2, 3)]),
            product(2, "チケット召喚", "召喚補助チケット５枚でカードを１枚召喚します。", 2, SUMMON_TICKET, [(5, 1), (0, 0)]),
        ],
    )
}

/// `get_product_shop_customize`: the costumes and decorations (`CustomizeShop.json`).
pub(super) fn shop_customize(update_num: i64) -> Json {
    let master = read_json("Params/game/CustomizeShop.json");
    let items = master
        .a("datalist")
        .iter()
        .map(|p| {
            Json::obj()
                .with("productId", p.i("id"))
                .with("startDate", p.i("startDate"))
                .with("endDate", p.i("endDate"))
                .with("newEndDate", p.i("newEndDate"))
                .with("productCategory", p.i("productCategory"))
                .with("productSubCategory", p.i("productSubCategory"))
                .with("productName", p.s("productName"))
                .with("productDescription", p.s("productDescription"))
                .with("itemId", p.i("itemId"))
                .with("consumeMaterialId", p.i("consumeMaterialId"))
                .with("consumeMaterialNum", p.i("consumeMaterialNum"))
                .with("equipMaterialId", p.i("equipMaterialId"))
                .with("equipMaterialNum", p.i("equipMaterialNum"))
                .with("iconName", p.s("iconName"))
                .with("viewPriority", p.i("viewPriority").to_string())
        })
        .collect();
    products(update_num, items)
}

/// `get_product_generate_card`: the cards made from materials (`masterShopGenerateCard.json`).
pub(super) fn generate_card(update_num: i64) -> Json {
    let master = read_json("Params/game/masterShopGenerateCard.json");
    let items = master
        .a("masterShopGenerateCard")
        .iter()
        .map(|p| {
            Json::obj()
                .with("cardId", p.i("cardId"))
                .with("optionId", p.i("optionId"))
                .with("startDate", p.i("startDate"))
                .with("endDate", p.i("endDate"))
                .with("newEndDate", p.i("newEndDate"))
                .with("materialId", p.i("consumeMaterialId"))
                .with("materialNum", p.i("consumeMaterialNum"))
                .with("viewPriority", p.s("viewPriority"))
        })
        .collect();
    products(update_num, items)
}

/// The material a generated card costs (`masterShopGenerateCard.json`).
pub(super) fn generate_cost(card_id: i64, option_id: i64) -> Option<(i64, i64)> {
    let master = read_json("Params/game/masterShopGenerateCard.json");
    master
        .a("masterShopGenerateCard")
        .iter()
        .find(|p| p.i("cardId") == card_id && p.i("optionId") == option_id)
        .map(|p| (p.i("consumeMaterialId"), p.i("consumeMaterialNum")))
}

/// The cost of a customize product (`CustomizeShop.json`): its purchase, its equipment.
pub(super) fn customize_cost(product_id: i64) -> Option<((i64, i64), (i64, i64))> {
    let master = read_json("Params/game/CustomizeShop.json");
    master
        .a("datalist")
        .iter()
        .find(|p| p.i("id") == product_id)
        .map(|p| ((p.i("consumeMaterialId"), p.i("consumeMaterialNum")), (p.i("equipMaterialId"), p.i("equipMaterialNum"))))
}

/// The players' cards (`hkb_card.csv`: the six classes' cards, 101000-106999; the summoned
/// units, tokens and other cards have other ids).
pub(super) fn cards() -> Vec<i64> {
    let text = read("Params/battle/hkb_card.csv").unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split(',').next()?.trim().parse::<i64>().ok())
        .filter(|id| (101_000..=106_999).contains(id))
        .collect()
}

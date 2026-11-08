//! `Factory/*`: the garage (ARTEMiS's season 2 `factory_controller.py`): the cars, buying one,
//! favourites (pickup), dress up and customization, the avatar, the avatar gacha's tokens,
//! renaming, the license plate number lottery.

use wal_payload_common::json::Json;

use super::store::{self, arr, merge, obj, pick, val};
use super::user::FULLTUNE_TICKET;
use super::{Ctx, now, ok};

/// Avatar tokens (`RewardCategoryEnum.AVATAR_TOKEN`).
const AVATAR_TOKEN: i64 = 5;

/// `NameChangeCategoryEnum`.
const CREDIT_RETRY: i64 = 1;
const DENIED: i64 = 3;

/// A car's customization (`UpdateCustomizeResult`, each of `UpdateMultipleCustomizeResult`).
const CUSTOMIZE: [&str; 9] = [
    "color",
    "color_stock_list",
    "color_stock_new_list",
    "parts_stock_list",
    "parts_stock_new_list",
    "parts_set_equip_list",
    "parts_list",
    "equip_parts_count",
    "style_car_id",
];

fn tickets(doc: &Json) -> Vec<Json> {
    doc.a("tickets").iter().map(|t| Json::obj().with("ticket_id", val(t, "ticket_id", 0i64)).with("ticket_cnt", val(t, "ticket_cnt", 0i64))).collect()
}

/// The favourites (`pickup_on_car_ids`, `pickup_off_car_ids`).
fn pickup(doc: &mut Json, req: &Json, skip: Option<i64>) {
    for on in req.a("pickup_on_car_ids") {
        if Some(on.i("style_car_id")) == skip {
            continue;
        }
        if let Some(car) = store::car_mut(doc, on.i("style_car_id")) {
            car.set("pickup_seq", val(on, "pickup_seq", 0i64));
        }
    }
    for off in req.a("pickup_off_car_ids") {
        if let Some(car) = store::car_mut(doc, off.i("style_car_id")) {
            car.set("pickup_seq", 0i64);
        }
    }
}

fn customize(doc: &mut Json, car: &Json) {
    if let Some(c) = store::car_mut(doc, car.i("style_car_id")) {
        merge(c, &pick(car, &CUSTOMIZE), false, &[]);
    }
}

/// The lottery's today count and won numbers (ARTEMiS's `calculate_lottery_info`: bit n of
/// `saved_value` is n*1111 won).
fn lottery_info(doc: &Json) -> Json {
    let lottery = doc.get("lottery");
    let count = lottery.filter(|l| now().saturating_sub(l.u("create_date")) < 86400).map_or(0, |l| l.i("lottery_count"));
    let saved = lottery.map_or(0, |l| l.i("saved_value"));
    let wins: Vec<Json> = (0..10i64).filter(|n| saved >> n & 1 != 0).map(|n| Json::obj().with("m_number_lottery_schedule_no", 1u32).with("win_number", n * 1111)).collect();
    Json::obj().with("lottery_count", count).with("win_list", wins)
}

/// `Factory/<endpoint>`.
pub(super) fn reply(ctx: &Ctx, endpoint: &str) -> Option<Json> {
    if !matches!(
        endpoint,
        "getcardata"
            | "avatargacharesult"
            | "buycarresult"
            | "savefavoritecar"
            | "updatecustomizeresult"
            | "updatemultiplecustomizeresult"
            | "updatecustomizeuser"
            | "updatecustomizeavatar"
            | "renamebefore"
            | "renameresult"
            | "numberlotterybefore"
            | "numberlotteryresult"
    ) {
        return None;
    }
    let Some(mut doc) = store::load(ctx.user) else { return Some(Json::obj().with("status_code", "E1001")) };
    let req = ctx.body;
    let doc = &mut doc;
    let resp = match endpoint {
        "getcardata" => {
            let cars: Vec<Json> = doc.a("cars").iter().map(|c| store::car_out(ctx.user, c, true)).collect();
            return Some(ok().with("car_data", cars));
        }
        "numberlotterybefore" => return Some(ok().with("lottery_info", lottery_info(doc))),
        "avatargacharesult" => {
            if let Some(stock) = req.get("stock_obj") {
                merge(obj(doc, "stock"), stock, true, &[]);
            }
            store::use_tickets(doc, AVATAR_TOKEN, req.i("use_ticket_cnt"));
            ok().with("ticket_data", tickets(doc))
        }
        "buycarresult" => {
            let style = req.i("style_car_id");
            let mut car = store::new_car(req);
            let purchase = doc.a("cars").iter().map(|c| c.i("purchase_seq")).max().unwrap_or(0) + 1;
            let seq = req.a("pickup_on_car_ids").iter().find(|c| c.i("style_car_id") == style).map_or(0, |c| c.i("pickup_seq"));
            car.set("tune_level", if req.i("use_ticket") != 0 { 16i64 } else { 1 });
            car.set("pickup_seq", seq);
            car.set("purchase_seq", purchase);
            store::upsert(arr(doc, "cars"), &["style_car_id"], car);
            pickup(doc, req, Some(style));
            if req.i("use_ticket") != 0 {
                store::use_tickets(doc, FULLTUNE_TICKET, 1);
            }
            let total_parts = val(doc.get("profile").unwrap_or(&Json::obj()), "total_car_parts_count", 0i64);
            let cars = doc.a("cars");
            ok().with("ticket_data", tickets(doc))
                .with("fulltune_count", store::fulltune_count(cars))
                .with("total_car_parts_count", total_parts)
                .with("car_layout_count", store::car_layout_count(cars))
                .with("car_style_count", store::car_style_count(cars))
        }
        "savefavoritecar" => {
            pickup(doc, req, None);
            ok()
        }
        "updatecustomizeresult" | "updatemultiplecustomizeresult" => {
            if let Some(n) = req.get("total_car_parts_count") {
                obj(doc, "profile").set("total_car_parts_count", n.clone());
            }
            if endpoint == "updatecustomizeresult" {
                customize(doc, req);
            } else {
                for car in req.a("car_list") {
                    customize(doc, car);
                }
            }
            store::put_tickets(doc, req.a("ticket_data"));
            ok()
        }
        "updatecustomizeuser" => {
            let fields = pick(
                req,
                &[
                    "mytitle_id",
                    "mytitle_efffect_id",
                    "papercup_id",
                    "tachometer_id",
                    "aura_id",
                    "aura_color_id",
                    "aura_line_id",
                    "bgm_id",
                    "keyholder_id",
                    "start_menu_bg_id",
                    "stamp_key_assign_0",
                    "stamp_key_assign_1",
                    "stamp_key_assign_2",
                    "stamp_key_assign_3",
                ],
            );
            merge(obj(doc, "profile"), &fields, false, &[]);
            if let Some(stock) = req.get("stock_obj") {
                merge(obj(doc, "stock"), stock, true, &[]);
            }
            ok()
        }
        "updatecustomizeavatar" => {
            if let Some(stock) = req.get("stock_obj") {
                merge(obj(doc, "stock"), stock, true, &[]);
            }
            if let Some(avatar) = req.get("avatar_obj").filter(|a| matches!(a, Json::Obj(_))) {
                doc.set("avatar", avatar.clone());
            }
            ok()
        }
        "renamebefore" | "renameresult" => {
            let p = obj(doc, "profile");
            if p.i("name_change_category") == DENIED {
                // StatusCodeEnum.BAD_REQUEST
                return Some(Json::obj().with("status_code", "E4002"));
            }
            if endpoint == "renamebefore" {
                p.set("name_change_category", CREDIT_RETRY);
            } else {
                p.set("username", val(req, "username", ""));
                p.set("name_change_category", DENIED);
            }
            ok()
        }
        _ => {
            // numberlotteryresult
            let mut lottery = doc.get("lottery").cloned().unwrap_or_else(|| Json::obj().with("saved_value", 0i64).with("create_date", now()));
            if req.i("win_number") != 10000 {
                lottery.set("saved_value", lottery.i("saved_value") | 1 << (req.i("win_number") / 1111).clamp(0, 62));
            }
            if now().saturating_sub(lottery.u("create_date")) > 86400 {
                lottery.set("create_date", now());
            }
            lottery.set("lottery_count", val(req, "lottery_count", 0i64));
            doc.set("lottery", lottery);
            if req.i("l_no") != 10000 && req.i("isEnd") == 1 {
                if let Some(car) = store::car_mut(doc, req.i("style_car_id")) {
                    car.set("l_no", val(req, "l_no", 0i64));
                }
            }
            store::put_tickets(doc, req.a("ticket_data"));
            merge(obj(doc, "profile"), &pick(req, &["cash", "total_cash", "dressup_point", "avatar_point"]), false, &[]);
            ok()
        }
    };
    store::save(ctx.user, doc);
    Some(resp)
}

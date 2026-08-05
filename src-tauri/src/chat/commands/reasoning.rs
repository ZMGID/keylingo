use tauri::State;

use crate::chat::model_metadata::reasoning_efforts_for_model;
use crate::settings::ModelProvider;
use crate::state::AppState;

/// 由「每对话思考等级」解析出实际下发给模型的 `(thinking_enabled, thinking_level)`。
/// chat 不再跟随全局思考开关（全局开关只服务 lens / 快速翻译），未显式选档时落到默认档「high」。
/// - `"off"` → 强制关思考，`thinking_level=None`。适配器看到 `thinking_enabled=false` 后
///   **显式**发关闭信号（OpenAI Chat → `reasoning_effort:"none"`；DeepSeek/Kimi →
///   `thinking.type=disabled`；Responses → `reasoning.effort:"none"`），不能省略字段——
///   DeepSeek / 部分代理省略时默认 effort=high，等于白关。
/// - `"low"|"medium"|"high"|"xhigh"|"max"` → 开思考并带等级（适配器原样映射为
///   reasoning_effort / reasoning.effort / output_config.effort / thinkingLevel）。
/// - `None` 或其它未知值 → 默认档「high」（与前端 `ThinkingLevelSelector` 的 DEFAULT_LEVEL 一致）。
///
/// **这里是「模型有没有 effort 旋钮」的唯一门控**：`reasoning_efforts_for_model` 解析出空列表时
/// （用户在模型详情里清空，或模型库标了空数组：Anthropic 4.6 以下不认 `output_config.effort`、
/// GLM-4.7 / Kimi K2.x / 通义走别的机制），开思考但不带等级，四个适配器的 `if let Some(effort)`
/// 自然全部跳过。等级**是否被这个模型接受**同样只看这份数据（前端选择器渲染的就是它），
/// 选错档直接吃 provider 的 400，不做静默收敛。
pub(crate) fn resolve_thinking(
    conv_level: Option<&str>,
    _global_enabled: bool,
    provider: Option<&ModelProvider>,
    model: &str,
) -> (bool, Option<String>) {
    if conv_level == Some("off") {
        return (false, None);
    }
    if reasoning_efforts_for_model(provider, model).is_empty() {
        return (true, None);
    }
    let level = match conv_level {
        Some(level @ ("low" | "medium" | "high" | "xhigh" | "max")) => level,
        _ => "high",
    };
    (true, Some(level.to_string()))
}

/// 返回某模型支持的思考等级列表（用户覆盖 → 模型库 → 家族兜底）。供前端等级选择器决定显示哪些档。
/// 传 `provider_id` 而不是 api_format：override 挂在 provider 上，api_format 也能就地取到。
#[tauri::command]
pub(crate) fn chat_reasoning_efforts_for_model(
    state: State<'_, AppState>,
    model: String,
    provider_id: Option<String>,
) -> Vec<String> {
    let settings = state.settings_read();
    let provider = provider_id
        .as_deref()
        .and_then(|id| settings.get_provider(id));
    reasoning_efforts_for_model(provider, &model)
}

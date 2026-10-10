# Spec 初版：技能参考资源

独立Spec累计基准 `origin/main=4b2357fcf01f47ba08d7724edbba7accfb60972c`，fresh `fa21243a..55068dc7`，继承历史结论，不声称穷尽重审。

固定初版 `55068dc7a5a07429525cff5d6237b3d28c663af3` 开放一项P2：规格允许总长256 UTF-8字节/16组件，却复用identifier的128字节组件上限。`references/` 加129个ASCII字符仅140字节/两组件，真实普通文件仍被拒绝。冻结初版binary SHA256 `d13d67c983b90a3461a3a76a61e4ad021cdb38955d5a07bb6b1ebd280aa24a88` 的真实CLI RED见 `/tmp/jiaclaw-oct11-resource-spec-initial.log`。

固定 `5a5cc77f9914f78810994a2883ef75b45c132577` 复查确认路径P2已修正、旧负例转绿，夹具仍有两项P2：长leaf未清理污染后续父链接矩阵，实际Directory not empty；最终严格日志扫描先于host停止，重复父PR已确认writer竞争模式并遗漏shutdown。资源位置的UTF-8异常未实际观察，不扩大为生产故障。

名称/正文与声明同锁选择、锁外能力读、字节hash、JSON预算、默认开关、原权限及实际worker所有权无其他新增缺失或范围扩张。初版和修复中失败分别保留，见 [终审](2026-10-11-skill-resources-spec.md)。

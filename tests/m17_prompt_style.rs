use devcode::Profile;

#[test]
fn every_profile_prompt_asks_for_brief_replies_and_no_unrequested_work() {
    for profile in [Profile::Ask, Profile::Plan, Profile::Build] {
        let prompt = profile.system_prompt();
        assert!(prompt.contains("one short line"), "{}", profile.name());
        assert!(
            prompt.contains("no lists of what you can do"),
            "{}",
            profile.name()
        );
        assert!(prompt.contains("Do not explore"), "{}", profile.name());
    }
}

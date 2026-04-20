#!/usr/bin/env python3
"""Analyze disposal-fix offender retest results."""

import json
from pathlib import Path


def main() -> None:
    # Load the pre-fix report (from offender_retest_report.json)
    pre_fix_path = Path("outputs/offender_retest_report.json")
    pre_fix_data = json.loads(pre_fix_path.read_text(encoding="utf-8"))
    pre_fix_per_file = pre_fix_data.get("per_file_results", {})

    # Load the post-fix results (from disposal_fix_offender_results.json)
    post_fix_path = Path("outputs/disposal_fix_offender_results.json")
    post_fix_data = json.loads(post_fix_path.read_text(encoding="utf-8"))

    print("=" * 100)
    print("DISPOSAL-FIX OFFENDER RETEST ANALYSIS")
    print("=" * 100)
    print()

    offender_files = [
        "trapezius_animation_small2",
        "galilean_moon_laplace_resonance_animation_2",
        "790106_0203_voyager_58m_to_31m_reduced",
    ]

    profiles = [
        "gifsicle_baseline",
        "rusticle_default",
        "rusticle_optimized_global",
    ]

    # Per-file analysis
    for name in offender_files:
        print(f"\n{'=' * 100}")
        print(f"FILE: {name}")
        print(f"{'=' * 100}")

        pre_file = pre_fix_per_file.get(name, {})
        post_file = post_fix_data.get(name, {})

        for profile in profiles:
            print(f"\n{profile}:")
            print("-" * 100)

            # Pre-fix data (from offender_retest_report.json post_fix section)
            pre_profile = pre_file.get("post_fix", {}).get(profile, {})
            pre_quality = pre_profile.get("quality", {})
            pre_error = pre_profile.get("quality_error")
            pre_runtime = pre_profile.get("median_runtime_ms")
            pre_bytes = pre_profile.get("median_output_bytes")

            # Post-fix data (from disposal_fix_offender_results.json)
            post_profile = post_file.get("profiles", {}).get(profile, {})
            post_quality = post_profile.get("quality", {})
            post_error = post_profile.get("quality_error")
            post_runtime = post_profile.get("median_runtime_ms")
            post_bytes = post_profile.get("median_output_bytes")

            # Print pre-fix
            print(f"  PRE-FIX:")
            if pre_error:
                print(f"    ERROR: {pre_error}")
            else:
                print(f"    avg_psnr: {pre_quality.get('avg_psnr', 'N/A')}")
                print(f"    avg_ssim: {pre_quality.get('avg_ssim', 'N/A')}")
                print(f"    avg_ba: {pre_quality.get('avg_ba', 'N/A')}")
                print(f"    worst_ba: {pre_quality.get('worst_ba', 'N/A')}")
            print(f"    runtime_ms: {pre_runtime}")
            print(f"    output_bytes: {pre_bytes}")

            # Print post-fix
            print(f"  POST-FIX:")
            if post_error:
                print(f"    ERROR: {post_error}")
            else:
                print(f"    avg_psnr: {post_quality.get('avg_psnr', 'N/A')}")
                print(f"    avg_ssim: {post_quality.get('avg_ssim', 'N/A')}")
                print(f"    avg_ba: {post_quality.get('avg_ba', 'N/A')}")
                print(f"    worst_ba: {post_quality.get('worst_ba', 'N/A')}")
            print(f"    runtime_ms: {post_runtime}")
            print(f"    output_bytes: {post_bytes}")

            # Compute deltas
            print(f"  DELTAS:")
            if pre_error and not post_error:
                print(f"    ✓ FIXED: Quality measurement error resolved!")
                print(f"    avg_ba now: {post_quality.get('avg_ba', 'N/A')}")
                print(f"    worst_ba now: {post_quality.get('worst_ba', 'N/A')}")
            elif not pre_error and not post_error:
                pre_ba = pre_quality.get("avg_ba")
                post_ba = post_quality.get("avg_ba")
                if pre_ba is not None and post_ba is not None:
                    ba_delta = post_ba - pre_ba
                    print(f"    avg_ba delta: {ba_delta:+.2f}")
                    if abs(ba_delta) > 1.0:
                        print(f"    ⚠ Significant BA change")
                    else:
                        print(f"    ✓ Stable BA")

                pre_worst = pre_quality.get("worst_ba")
                post_worst = post_quality.get("worst_ba")
                if pre_worst is not None and post_worst is not None:
                    worst_delta = post_worst - pre_worst
                    print(f"    worst_ba delta: {worst_delta:+.2f}")

            if pre_runtime is not None and post_runtime is not None:
                runtime_delta = post_runtime - pre_runtime
                print(f"    runtime_ms delta: {runtime_delta:+.2f}")

            if pre_bytes is not None and post_bytes is not None:
                bytes_delta = post_bytes - pre_bytes
                print(f"    output_bytes delta: {bytes_delta:+d}")

    # Summary
    print(f"\n{'=' * 100}")
    print("SUMMARY")
    print(f"{'=' * 100}")
    print()

    # Check for catastrophic improvements
    print("Catastrophic BA Improvements:")
    print("-" * 100)
    for name in offender_files:
        pre_file = pre_fix_per_file.get(name, {})
        post_file = post_fix_data.get(name, {})

        for profile in ["rusticle_default", "rusticle_optimized_global"]:
            pre_profile = pre_file.get("post_fix", {}).get(profile, {})
            pre_ba = pre_profile.get("quality", {}).get("avg_ba")
            pre_error = pre_profile.get("quality_error")

            post_profile = post_file.get("profiles", {}).get(profile, {})
            post_ba = post_profile.get("quality", {}).get("avg_ba")
            post_error = post_profile.get("quality_error")

            if pre_error and not post_error:
                print(f"✓ {name} / {profile}: FIXED (was error, now avg_ba={post_ba})")
            elif pre_ba is not None and post_ba is not None:
                ba_delta = post_ba - pre_ba
                if ba_delta < -5.0:
                    print(f"✓ {name} / {profile}: Improved by {abs(ba_delta):.2f} BA")
                elif ba_delta > 5.0:
                    print(f"⚠ {name} / {profile}: Regressed by {ba_delta:.2f} BA")

    print()
    print("Remaining Divergences >1.0 BA vs gifsicle:")
    print("-" * 100)
    for name in offender_files:
        post_file = post_fix_data.get(name, {})
        gifsicle_ba = (
            post_file.get("profiles", {})
            .get("gifsicle_baseline", {})
            .get("quality", {})
            .get("avg_ba")
        )

        for profile in ["rusticle_default", "rusticle_optimized_global"]:
            rusticle_ba = (
                post_file.get("profiles", {})
                .get(profile, {})
                .get("quality", {})
                .get("avg_ba")
            )
            if gifsicle_ba is not None and rusticle_ba is not None:
                divergence = rusticle_ba - gifsicle_ba
                if divergence > 1.0:
                    print(
                        f"⚠ {name} / {profile}: {divergence:.2f} BA divergence from gifsicle"
                    )

    print()
    print("Voyager Measurement Behavior:")
    print("-" * 100)
    voyager_post = post_fix_data.get("790106_0203_voyager_58m_to_31m_reduced", {})
    for profile in profiles:
        post = voyager_post.get("profiles", {}).get(profile, {})
        quality_error = post.get("quality_error")
        if quality_error:
            print(f"✗ {profile}: ERROR - {quality_error}")
        else:
            quality = post.get("quality", {})
            print(
                f"✓ {profile}: Valid (avg_ba={quality.get('avg_ba', 'N/A')}, worst_ba={quality.get('worst_ba', 'N/A')})"
            )

    print()


if __name__ == "__main__":
    main()

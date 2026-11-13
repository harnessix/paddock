# Homebrew formula for paddock. It builds from the source tarball of a
# release. The release workflow fills in the version, the checksums, the
# repository and the guest files of the VM, and attaches the result to the
# release as paddock.rb. Copy that file into a tap.
class Paddock < Formula
  desc "Run any coding-agent harness with full auto-approval inside an isolated sandbox"
  homepage "https://github.com/@REPO@"
  url "https://github.com/@REPO@/releases/download/v@VERSION@/paddock-@VERSION@.tar.gz"
  sha256 "@SHA256@"
  license "MIT"

  # The terminal interface (tui/) becomes one self-contained program,
  # paddock-tui, with `bun build --compile`: the same form as in a Linux
  # release, so paddock finds and starts it in the same way. bun and node
  # are in homebrew-core, and both are needed for the build only: node has
  # npm, which installs the packages of tui/package-lock.json with their
  # checksums, and it runs the TypeScript compiler. bun then puts the result
  # and its own runtime into one file. With node alone, paddock-tui would
  # be a script, and node would become a dependency of every install.
  depends_on "bun" => :build
  depends_on "node" => :build
  depends_on "rust" => :build

  # The guest files of the VM backend. The release workflow takes the URLs
  # and sha256 values from packaging/vm/assets.toml.
  on_macos do
    on_arm do
      resource "vm-kernel" do
        url "@VM_KERNEL_URL@"
        sha256 "@VM_KERNEL_SHA256@"
      end

      resource "vm-rootfs" do
        url "@VM_ROOTFS_URL@"
        sha256 "@VM_ROOTFS_SHA256@"
      end
    end
  end

  def install
    # paddock update and paddock doctor read the releases of this repository.
    ENV["PADDOCK_RELEASE_REPO"] = "@REPO@"
    # The source tarball of the release has no git checkout. This marks the
    # build as the release, so `paddock --version` prints only the version.
    ENV["PADDOCK_RELEASE_BUILD"] = "1"
    system "cargo", "install", *std_cargo_args

    # paddock-tui next to paddock. No package script runs. The target is
    # this machine, so bun copies its own runtime and downloads no other.
    cd "tui" do
      system "npm", "ci", "--ignore-scripts"
      system "npm", "run", "build"
      system "bun", "scripts/bun-build.mjs", "native", bin/"paddock-tui"
    end
    return unless OS.mac?

    # paddock-vm owns the Linux VM of the VM backend. The Virtualization
    # framework needs the virtualization entitlement, and an ad hoc
    # signature carries it. Without it, a VM start fails.
    system "codesign", "--force", "--sign", "-",
           "--entitlements", "packaging/macos/paddock-vm.entitlements",
           bin/"paddock-vm"
    return unless Hardware::CPU.arm?

    # paddock-guest is the agent inside paddock's macOS guest. A session
    # shares it read-only, and the agent in the guest replaces itself with
    # it when the versions differ, after a check of this hash. So a paddock
    # upgrade never needs a rebuild of the golden image.
    system "codesign", "--force", "--sign", "-", bin/"paddock-guest"
    (bin/"paddock-guest.sha256").write("#{Digest::SHA256.file(bin/"paddock-guest").hexdigest}\n")

    # paddock finds the guest files here when `paddock vm setup` did not
    # install its own copy.
    (pkgshare/"vm").install "packaging/vm/assets.toml"
    # A staged file keeps the last part of its URL: kernel, rootfs.img.
    resource("vm-kernel").stage { (pkgshare/"vm").install "kernel" }
    resource("vm-rootfs").stage { (pkgshare/"vm").install "rootfs.img" }
  end

  test do
    assert_match "paddock #{version}", shell_output("#{bin}/paddock --version")
    # A paddock-tui that lost its code prints the version of bun instead.
    assert_equal "paddock-tui #{version}", shell_output("#{bin}/paddock-tui --version").strip
    if OS.mac?
      assert_match "com.apple.security.virtualization",
                   shell_output("codesign -d --entitlements - #{bin}/paddock-vm 2>&1")
      if Hardware::CPU.arm?
        assert_equal Digest::SHA256.file(bin/"paddock-guest").hexdigest,
                     (bin/"paddock-guest.sha256").read.strip
      end
    end
  end
end

import {
  loadAvatarRuntime,
  type AvatarData,
  type RuntimeAvatar,
} from "./avatars/avatar-runtime";
import { avatarData as cubeeData } from "./avatars/cubee.avatar";
import { avatarData as oneeData } from "./avatars/onee.avatar";
import type { BotStateName } from "../core/layout";
import type { AvatarChoice } from "../core/state";
export type { AvatarChoice } from "../core/state";

type CustomAvatar = Exclude<AvatarChoice, "macha">;
type AvatarAnimation = "idle" | "working" | "thinking" | "searching" | "listening" | "happy";
type AnimationData = AvatarData<AvatarAnimation>;

interface AvatarDefinition {
  data: AnimationData;
  label: string;
}

export interface AvatarDisplay {
  setState(state: BotStateName): void;
  setLook(x: number, y: number): void;
  destroy(): void;
}

const DEFINITIONS: Record<CustomAvatar, AvatarDefinition> = {
  cubee: { data: cubeeData, label: "Cubee" },
  onee: { data: oneeData, label: "Onee" },
};

const RUNTIME_CACHE = new Map<CustomAvatar, ReturnType<typeof loadAvatarRuntime<AvatarAnimation>>>();
const DISPLAYS = new Set<MountedAvatar>();
let selectedAvatar: AvatarChoice = "macha";
let selectedState: BotStateName = "idle";

function animationForState(state: BotStateName): AvatarAnimation {
  switch (state) {
    case "working":
    case "approval":
      return "working";
    case "thinking":
    case "question":
      return "thinking";
    case "searching":
      return "searching";
    case "finished":
      return "happy";
    case "sleeping":
      return "listening";
    default:
      return "idle";
  }
}

function runtimeFor(choice: CustomAvatar) {
  let runtime = RUNTIME_CACHE.get(choice);
  if (!runtime) {
    runtime = loadAvatarRuntime<AvatarAnimation>(DEFINITIONS[choice].data);
    RUNTIME_CACHE.set(choice, runtime);
  }
  return runtime;
}

class MountedAvatar implements AvatarDisplay {
  private currentChoice: AvatarChoice | null = null;
  private runtimeAvatar: RuntimeAvatar<AvatarAnimation> | null = null;
  private eyesLayer: SVGGElement | null = null;
  private currentAnimation: AvatarAnimation | null = null;
  private generation = 0;
  private state = selectedState;
  private lookX = 0;
  private lookY = 0;

  constructor(
    private readonly host: HTMLElement,
    private readonly forcedChoice?: AvatarChoice,
  ) {
    host.classList.add("custom-avatar-host");
    host.setAttribute("aria-hidden", "true");
    this.sync();
  }

  setState(state: BotStateName) {
    this.state = state;
    const animation = animationForState(state);
    if (this.runtimeAvatar && animation !== this.currentAnimation) {
      this.currentAnimation = animation;
      this.runtimeAvatar.play(animation);
    }
  }

  setLook(x: number, y: number) {
    this.lookX = Number.isFinite(x) ? Math.max(-1, Math.min(1, x)) : 0;
    this.lookY = Number.isFinite(y) ? Math.max(-1, Math.min(1, y)) : 0;
    this.applyLook();
  }

  private applyLook() {
    if (!this.eyesLayer) return;
    const x = this.lookX * 14;
    const y = this.lookY * -10;
    this.eyesLayer.setAttribute("transform", `translate(${x.toFixed(2)} ${y.toFixed(2)})`);
  }

  destroy() {
    this.generation += 1;
    this.runtimeAvatar?.destroy();
    this.runtimeAvatar = null;
    this.eyesLayer = null;
    this.currentAnimation = null;
    this.host.replaceChildren();
    this.host.remove();
    DISPLAYS.delete(this);
  }

  sync() {
    const choice = this.forcedChoice ?? selectedAvatar;
    this.host.dataset.avatarChoice = choice;
    this.host.style.display = choice === "macha" ? "none" : "block";
    if (choice === this.currentChoice) {
      this.setState(selectedState);
      return;
    }

    this.currentChoice = choice;
    const generation = ++this.generation;
    this.runtimeAvatar?.destroy();
    this.runtimeAvatar = null;
    this.eyesLayer = null;
    this.currentAnimation = null;
    delete this.host.dataset.avatarError;
    this.host.replaceChildren();
    if (choice === "macha") return;

    void runtimeFor(choice).then((runtime) => {
      if (
        generation !== this.generation ||
        !this.host.isConnected ||
        (this.forcedChoice ?? selectedAvatar) !== choice
      ) return;
      this.runtimeAvatar = runtime.createAvatar(this.host, {
        animation: animationForState(this.state),
        autoplay: true,
        loop: true,
        size: "100%",
        onAnimationEnd: () => undefined,
      });
      this.eyesLayer = this.host.querySelector<SVGGElement>("svg g[clip-path]");
      this.applyLook();
      this.currentAnimation = animationForState(this.state);
    }).catch((error: unknown) => {
      if (generation !== this.generation) return;
      console.error(`[macha] could not load the ${DEFINITIONS[choice].label} avatar`, error);
      this.host.dataset.avatarError = "true";
    });
  }
}

export function mountAvatar(host: HTMLElement, choice?: AvatarChoice): AvatarDisplay {
  const display = new MountedAvatar(host, choice);
  DISPLAYS.add(display);
  return display;
}

export function setAvatarSelection(choice: AvatarChoice) {
  selectedAvatar = choice;
  for (const display of DISPLAYS) display.sync();
}

export function setAvatarState(state: BotStateName) {
  selectedState = state;
  for (const display of DISPLAYS) display.setState(state);
}

export function currentAvatarSelection(): AvatarChoice {
  return selectedAvatar;
}

export function avatarLabel(choice: AvatarChoice): string {
  switch (choice) {
    case "cubee":
      return DEFINITIONS.cubee.label;
    case "onee":
      return DEFINITIONS.onee.label;
    default:
      return "Macha";
  }
}
